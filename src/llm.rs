//! LLM provider abstraction.
//!
//! Two backends:
//! - `Ollama` — local, always available when `ollama serve` is running.
//! - `Pi`     — online via the `pi` CLI (OpenAI Codex / gpt-5.4-mini by default).
//!
//! Selection order (unless forced via `Mode`):
//! 1. If `CIRI_OFFLINE=1` → force Ollama.
//! 2. If `CIRI_ONLINE=1`  → force Pi.
//! 3. Quick TCP probe to the internet (1.1.1.1:443, 300ms). Online → Pi, offline → Ollama.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::time::timeout;

pub struct Message<'a> {
    pub role: &'a str,
    pub content: &'a str,
}

#[derive(Debug, Clone, Copy)]
pub enum Mode {
    Auto,
    ForceOffline,
    ForceOnline,
}

#[async_trait]
pub trait Llm: Send + Sync {
    fn name(&self) -> String;

    /// Stream a chat response to `sink`. `on_first_token` fires once before the first byte.
    /// Returns full accumulated content.
    async fn chat(
        &self,
        messages: &[Message<'_>],
        temperature: f32,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
        on_first_token: &mut (dyn FnMut() + Send),
    ) -> Result<String>;
}

// ---------------- Ollama ----------------

pub struct Ollama {
    pub host: String,
    pub model: String,
}

#[derive(Serialize)]
struct OllamaReq<'a> {
    model: &'a str,
    messages: Vec<OllamaMsg<'a>>,
    stream: bool,
    options: OllamaOpts,
}
#[derive(Serialize)]
struct OllamaMsg<'a> {
    role: &'a str,
    content: &'a str,
}
#[derive(Serialize)]
struct OllamaOpts {
    temperature: f32,
}
#[derive(Deserialize)]
struct OllamaChunk {
    #[serde(default)]
    message: Option<OllamaChunkMsg>,
    #[serde(default)]
    done: bool,
}
#[derive(Deserialize)]
struct OllamaChunkMsg {
    #[serde(default)]
    content: Option<String>,
}

#[async_trait]
impl Llm for Ollama {
    fn name(&self) -> String {
        format!("ollama:{}", self.model)
    }

    async fn chat(
        &self,
        messages: &[Message<'_>],
        temperature: f32,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
        on_first_token: &mut (dyn FnMut() + Send),
    ) -> Result<String> {
        let url = format!("{}/api/chat", self.host.trim_end_matches('/'));
        let body = OllamaReq {
            model: &self.model,
            messages: messages
                .iter()
                .map(|m| OllamaMsg { role: m.role, content: m.content })
                .collect(),
            stream: true,
            options: OllamaOpts { temperature },
        };
        let client = reqwest::Client::new();
        let resp = client.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let s = resp.status();
            let t = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Ollama error {s}: {t}"));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = Vec::<u8>::new();
        let mut out = String::new();
        let mut started = false;

        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            buf.extend_from_slice(&bytes);
            while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                let line = &line[..line.len() - 1];
                if line.is_empty() {
                    continue;
                }
                let parsed: OllamaChunk = match serde_json::from_slice(line) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                if let Some(m) = parsed.message {
                    if let Some(c) = m.content {
                        if !c.is_empty() {
                            if !started {
                                on_first_token();
                                started = true;
                            }
                            sink.write_all(c.as_bytes()).await?;
                            sink.flush().await?;
                            out.push_str(&c);
                        }
                    }
                }
                if parsed.done {
                    return Ok(out);
                }
            }
        }
        Ok(out)
    }
}

// ---------------- pi CLI ----------------

pub struct Pi {
    pub provider: String,
    pub model: String,
}

impl Default for Pi {
    fn default() -> Self {
        Self {
            provider: "openai-codex".into(),
            model: "gpt-5.4-mini".into(),
        }
    }
}

#[async_trait]
impl Llm for Pi {
    fn name(&self) -> String {
        format!("pi:{}/{}", self.provider, self.model)
    }

    async fn chat(
        &self,
        messages: &[Message<'_>],
        _temperature: f32,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
        on_first_token: &mut (dyn FnMut() + Send),
    ) -> Result<String> {
        // Split messages into a system prompt (concatenated) and a single user prompt.
        let mut system = String::new();
        let mut user = String::new();
        for m in messages {
            match m.role {
                "system" => {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(m.content);
                }
                _ => {
                    if !user.is_empty() {
                        user.push_str("\n\n");
                    }
                    user.push_str(m.content);
                }
            }
        }

        let mut cmd = Command::new("pi");
        cmd.args([
            "-p",
            "--provider",
            &self.provider,
            "--model",
            &self.model,
            "--no-tools",
            "--no-extensions",
            "--no-session",
            "--no-skills",
            "--no-prompt-templates",
        ]);
        if !system.is_empty() {
            cmd.arg("--system-prompt").arg(&system);
        }
        cmd.arg(&user);
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| anyhow!("spawn pi: {e}"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        let mut reader = BufReader::new(stdout);

        let mut out = String::new();
        let mut buf = [0u8; 1024];
        let mut started = false;
        use tokio::io::AsyncReadExt;
        loop {
            let n = reader.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            if !started {
                on_first_token();
                started = true;
            }
            sink.write_all(&buf[..n]).await?;
            sink.flush().await?;
            out.push_str(&String::from_utf8_lossy(&buf[..n]));
        }

        let status = child.wait().await?;
        if !status.success() {
            let mut stderr = String::new();
            if let Some(mut e) = child.stderr.take() {
                e.read_to_string(&mut stderr).await.ok();
            }
            return Err(anyhow!("pi failed: {stderr}"));
        }
        Ok(out)
    }
}

// ---------------- Selection ----------------

pub async fn is_online() -> bool {
    // Env overrides
    if std::env::var("CIRI_OFFLINE").ok().as_deref() == Some("1") {
        return false;
    }
    if std::env::var("CIRI_ONLINE").ok().as_deref() == Some("1") {
        return true;
    }
    // Fast TCP probe: prefer the pi backend host, fall back to a well-known IP.
    for addr in ["api.openai.com:443", "1.1.1.1:443"] {
        if timeout(Duration::from_millis(400), TcpStream::connect(addr))
            .await
            .ok()
            .and_then(|r| r.ok())
            .is_some()
        {
            return true;
        }
    }
    false
}

pub struct ProviderChoice {
    /// User-facing label, e.g. "ollama:gemma4:e2b" or "pi:openai-codex/gpt-5.4-mini".
    pub label: String,
    pub llm: Box<dyn Llm>,
}

/// Build the active provider. `offline_model` is the Ollama model to use when falling back.
pub async fn select(
    mode: Mode,
    ollama_host: &str,
    offline_model: &str,
    model_override: Option<&str>,
) -> ProviderChoice {
    let use_online = match mode {
        Mode::ForceOnline => true,
        Mode::ForceOffline => false,
        Mode::Auto => is_online().await,
    };

    if use_online {
        let pi = Pi {
            model: model_override.unwrap_or("gpt-5.4-mini").to_string(),
            ..Pi::default()
        };
        let label = pi.name();
        ProviderChoice {
            label,
            llm: Box::new(pi),
        }
    } else {
        let ollama = Ollama {
            host: ollama_host.to_string(),
            model: model_override.unwrap_or(offline_model).to_string(),
        };
        let label = ollama.name();
        ProviderChoice {
            label,
            llm: Box::new(ollama),
        }
    }
}
