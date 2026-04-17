//! Shared test helpers: build throwaway git repos, run a mock Ollama server,
//! and spawn the `ciri` binary with a controlled environment.

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

/// Path to the `ciri` binary built by cargo for the current test run.
pub fn ciri_bin() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by cargo for integration tests
    PathBuf::from(env!("CARGO_BIN_EXE_ciri"))
}

/// A scratch git repo with optional remote and initial commit on `main`.
pub struct Repo {
    pub dir: TempDir,
    pub remote_dir: TempDir,
}

impl Repo {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let remote_dir = tempfile::tempdir().unwrap();

        // bare remote
        run(remote_dir.path(), &["init", "--bare", "-b", "main"]);

        // working repo
        let p = dir.path();
        run(p, &["init", "-b", "main"]);
        run(p, &["config", "user.email", "test@example.com"]);
        run(p, &["config", "user.name", "Test"]);
        std::fs::write(p.join("README.md"), "initial\n").unwrap();
        run(p, &["add", "."]);
        run(p, &["commit", "-m", "chore: initial"]);
        run(
            p,
            &[
                "remote",
                "add",
                "origin",
                remote_dir.path().to_str().unwrap(),
            ],
        );
        run(p, &["push", "-u", "origin", "main"]);
        Self { dir, remote_dir }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn checkout_new(&self, branch: &str) {
        run(self.path(), &["checkout", "-b", branch]);
    }

    pub fn write(&self, rel: &str, contents: &str) {
        let p = self.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(p, contents).unwrap();
    }

    pub fn stage_all(&self) {
        run(self.path(), &["add", "-A"]);
    }

    pub fn commit(&self, msg: &str) {
        run(self.path(), &["commit", "-m", msg]);
    }

    pub fn head_message(&self) -> String {
        let out = Command::new("git")
            .args(["log", "-1", "--pretty=%B"])
            .current_dir(self.path())
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }
}

fn run(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Minimal mock of the Ollama `/api/chat` endpoint.
/// Streams a canned response as NDJSON chunks and exits on first request.
pub struct MockOllama {
    pub addr: SocketAddr,
    pub handle: JoinHandle<Option<String>>,
}

impl MockOllama {
    /// Serves `response_text` as a single content chunk. Returns the URL and a
    /// handle whose value is the raw request body received (for assertions).
    pub async fn start(response_text: &'static str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.ok()?;
            // Read request until we have headers + body. We don't need to be a
            // full HTTP parser — Content-Length tells us when we've got the body.
            let mut buf = Vec::<u8>::with_capacity(4096);
            let mut tmp = [0u8; 2048];
            let mut body_start = None;
            let mut content_len: Option<usize> = None;
            loop {
                let n = sock.read(&mut tmp).await.ok()?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
                if body_start.is_none() {
                    if let Some(i) = find_double_crlf(&buf) {
                        body_start = Some(i + 4);
                        let headers = std::str::from_utf8(&buf[..i]).unwrap_or("");
                        for line in headers.split("\r\n") {
                            if let Some(v) =
                                line.to_ascii_lowercase().strip_prefix("content-length:")
                            {
                                content_len = v.trim().parse().ok();
                            }
                        }
                    }
                }
                if let (Some(bs), Some(cl)) = (body_start, content_len) {
                    if buf.len() >= bs + cl {
                        break;
                    }
                }
            }

            let body = body_start
                .map(|bs| String::from_utf8_lossy(&buf[bs..]).to_string())
                .unwrap_or_default();

            // Build an NDJSON streaming response
            let chunks = [
                serde_json::json!({
                    "model": "mock",
                    "message": { "role": "assistant", "content": response_text },
                    "done": false
                }),
                serde_json::json!({
                    "model": "mock",
                    "message": { "role": "assistant", "content": "" },
                    "done": true
                }),
            ];
            let mut payload = String::new();
            for c in &chunks {
                payload.push_str(&c.to_string());
                payload.push('\n');
            }

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                payload.len(),
                payload
            );
            sock.write_all(response.as_bytes()).await.ok()?;
            sock.flush().await.ok()?;
            Some(body)
        });
        Self { addr, handle }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub async fn received_body(self) -> String {
        self.handle.await.unwrap().unwrap_or_default()
    }
}

fn find_double_crlf(b: &[u8]) -> Option<usize> {
    b.windows(4).position(|w| w == b"\r\n\r\n")
}
