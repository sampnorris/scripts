//! LLM provider abstraction.
//!
//! Two backends:
//! - `Ollama`    — local, always available when `ollama serve` is running.
//! - `OpenCode`  — online via the OpenCode server HTTP API (http://localhost:3456).
//!
//! Selection order (unless forced via `Mode`):
//! 1. If `CIRI_OFFLINE=1` → force Ollama.
//! 2. If `CIRI_ONLINE=1`  → force OpenCode.
//! 3. Quick TCP probe to localhost:3456. Running → OpenCode, not running → Ollama.

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
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
                .map(|m| OllamaMsg {
                    role: m.role,
                    content: m.content,
                })
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

// ---------------- OpenCode ----------------

pub struct OpenCode {
    pub host: String,
}

impl Default for OpenCode {
    fn default() -> Self {
        Self {
            host: "http://localhost:3456".into(),
        }
    }
}

#[derive(Deserialize)]
struct CreateSessionResp {
    id: String,
}

#[derive(Deserialize)]
struct MessageResp {
    parts: Vec<serde_json::Value>,
}

#[async_trait]
impl Llm for OpenCode {
    fn name(&self) -> String {
        "opencode".to_string()
    }

    async fn chat(
        &self,
        messages: &[Message<'_>],
        _temperature: f32,
        sink: &mut (dyn AsyncWrite + Unpin + Send),
        on_first_token: &mut (dyn FnMut() + Send),
    ) -> Result<String> {
        let client = reqwest::Client::new();

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

        let create_url = format!("{}/session", self.host.trim_end_matches('/'));
        let create_resp = client
            .post(&create_url)
            .json(&serde_json::json!({}))
            .send()
            .await
            .map_err(|e| anyhow!("OpenCode create session: {e}"))?;
        if !create_resp.status().is_success() {
            let s = create_resp.status();
            let t = create_resp.text().await.unwrap_or_default();
            return Err(anyhow!("OpenCode create session error {s}: {t}"));
        }
        let sess: CreateSessionResp = create_resp
            .json()
            .await
            .map_err(|e| anyhow!("OpenCode create session parse: {e}"))?;
        let session_id = sess.id;

        let msg_url = format!(
            "{}/session/{}/message",
            self.host.trim_end_matches('/'),
            session_id
        );
        let body = serde_json::json!({
            "system": system,
            "parts": [{ "type": "text", "text": user }]
        });
        let msg_result = client.post(&msg_url).json(&body).send().await;

        let delete_url = format!("{}/session/{}", self.host.trim_end_matches('/'), session_id);
        let _ = client.delete(&delete_url).send().await;

        let msg_resp = msg_result.map_err(|e| anyhow!("OpenCode message: {e}"))?;
        if !msg_resp.status().is_success() {
            let s = msg_resp.status();
            let t = msg_resp.text().await.unwrap_or_default();
            return Err(anyhow!("OpenCode message error {s}: {t}"));
        }
        let parsed: MessageResp = msg_resp
            .json()
            .await
            .map_err(|e| anyhow!("OpenCode message parse: {e}"))?;

        let mut out = String::new();
        for part in &parsed.parts {
            if part.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
        }

        if out.is_empty() {
            return Err(anyhow!("OpenCode returned no text parts in response"));
        }

        on_first_token();
        sink.write_all(out.as_bytes()).await?;
        sink.flush().await?;
        Ok(out)
    }
}

// ---------------- Selection ----------------

/// Pure decision: given the mode + connectivity probe, should we use the online backend?
pub fn resolve_use_online(mode: Mode, probe_online: bool) -> bool {
    match mode {
        Mode::ForceOnline => true,
        Mode::ForceOffline => false,
        Mode::Auto => probe_online,
    }
}

pub async fn is_online() -> bool {
    if std::env::var("CIRI_OFFLINE").ok().as_deref() == Some("1") {
        return false;
    }
    if std::env::var("CIRI_ONLINE").ok().as_deref() == Some("1") {
        return true;
    }
    // Fast TCP probe: check if the OpenCode server is running locally.
    timeout(
        Duration::from_millis(400),
        TcpStream::connect("127.0.0.1:3456"),
    )
    .await
    .ok()
    .and_then(|r| r.ok())
    .is_some()
}

pub struct ProviderChoice {
    /// User-facing label, e.g. "ollama:gemma4:e2b" or "opencode".
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
    let use_online = resolve_use_online(mode, is_online().await);

    if use_online {
        let oc = OpenCode::default();
        let label = oc.name();
        ProviderChoice {
            label,
            llm: Box::new(oc),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_online_wins() {
        assert!(resolve_use_online(Mode::ForceOnline, false));
    }
    #[test]
    fn force_offline_wins() {
        assert!(!resolve_use_online(Mode::ForceOffline, true));
    }
    #[test]
    fn auto_follows_probe() {
        assert!(resolve_use_online(Mode::Auto, true));
        assert!(!resolve_use_online(Mode::Auto, false));
    }

    #[test]
    fn offline_selection_uses_ollama_with_override() {
        let ollama = Ollama {
            host: "http://x".into(),
            model: "gemma4:e2b".into(),
        };
        assert_eq!(ollama.name(), "ollama:gemma4:e2b");
    }

    #[test]
    fn online_selection_uses_opencode_defaults() {
        let opencode = OpenCode::default();
        assert_eq!(opencode.host, "http://localhost:3456");
        assert_eq!(opencode.name(), "opencode");
    }
}
