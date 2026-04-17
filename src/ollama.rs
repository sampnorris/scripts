use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt};

#[derive(Serialize)]
pub struct ChatMessage<'a> {
    pub role: &'a str,
    pub content: &'a str,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage<'a>],
    stream: bool,
    options: ChatOptions,
}

#[derive(Serialize)]
struct ChatOptions {
    temperature: f32,
}

#[derive(Deserialize)]
struct ChatChunk {
    #[serde(default)]
    message: Option<ChunkMessage>,
    #[serde(default)]
    done: bool,
}

#[derive(Deserialize)]
struct ChunkMessage {
    #[serde(default)]
    content: Option<String>,
}

/// Stream a chat completion, writing tokens to `sink` as they arrive.
/// `on_first_token` fires once, right before the first visible token.
/// Returns full accumulated content.
pub async fn chat_stream<W, F>(
    host: &str,
    model: &str,
    messages: &[ChatMessage<'_>],
    temperature: f32,
    sink: &mut W,
    mut on_first_token: F,
) -> Result<String>
where
    W: AsyncWrite + Unpin,
    F: FnMut(),
{
    let url = format!("{}/api/chat", host.trim_end_matches('/'));
    let body = ChatRequest {
        model,
        messages,
        stream: true,
        options: ChatOptions { temperature },
    };

    let client = reqwest::Client::new();
    let resp = client.post(&url).json(&body).send().await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Ollama error {status}: {text}"));
    }

    let mut stream = resp.bytes_stream();
    let mut buf = Vec::<u8>::new();
    let mut out = String::new();
    let mut started = false;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk?;
        buf.extend_from_slice(&bytes);

        while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
            let line = buf.drain(..=nl).collect::<Vec<u8>>();
            let line = &line[..line.len() - 1];
            if line.is_empty() {
                continue;
            }
            let parsed: ChatChunk = match serde_json::from_slice(line) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Some(msg) = parsed.message {
                if let Some(c) = msg.content {
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
