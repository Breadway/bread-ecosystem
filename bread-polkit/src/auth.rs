//! PAM conversation with the polkit agent helper.

use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;

use bread_polkit::helper::{parse_helper_line, HelperLine, Transport};

/// Outcome of one helper conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Failure { message: Option<String> },
}

/// Handshake + PAM loop for one password attempt.
pub async fn authenticate(
    transport: &Transport,
    username: &str,
    cookie: &str,
    password: &str,
) -> Result<Outcome> {
    match transport {
        Transport::Socket(path) => {
            let mut stream = UnixStream::connect(path)
                .await
                .with_context(|| format!("connect {}", path.display()))?;
            stream.write_all(username.as_bytes()).await?;
            stream.write_all(b"\n").await?;
            stream.write_all(cookie.as_bytes()).await?;
            stream.write_all(b"\n").await?;
            let (reader, writer) = stream.into_split();
            converse(BufReader::new(reader), writer, password).await
        }
        Transport::Exec(path) => {
            let mut child = Command::new(path)
                .arg(username)
                .env("LC_ALL", "C")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .with_context(|| format!("spawn {}", path.display()))?;
            let mut stdin = child.stdin.take().context("polkit helper has no stdin")?;
            let stdout = child.stdout.take().context("polkit helper has no stdout")?;
            stdin.write_all(cookie.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            let outcome = converse(BufReader::new(stdout), stdin, password).await;
            let _ = child.wait().await;
            outcome
        }
    }
}

async fn converse<R, W>(mut reader: BufReader<R>, mut writer: W, password: &str) -> Result<Outcome>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut last_info: Option<String> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(Outcome::Failure {
                message: last_info.take(),
            });
        }
        match parse_helper_line(&line) {
            HelperLine::PromptEchoOff(_) => {
                writer.write_all(password.as_bytes()).await?;
                writer.write_all(b"\n").await?;
                writer.flush().await?;
            }
            HelperLine::PromptEchoOn(_) => {
                // Visible prompt (username, etc.) — we already sent the
                // identity in the handshake. An empty line is safer than
                // echoing the password.
                writer.write_all(b"\n").await?;
                writer.flush().await?;
            }
            HelperLine::ErrorMsg(msg) | HelperLine::TextInfo(msg) => {
                if !msg.is_empty() {
                    last_info = Some(msg);
                }
            }
            HelperLine::Success => return Ok(Outcome::Success),
            HelperLine::Failure => {
                return Ok(Outcome::Failure {
                    message: last_info.take(),
                });
            }
            HelperLine::Other(other) => {
                if !other.is_empty() {
                    tracing::debug!("helper: {other}");
                }
            }
        }
    }
}

/// Shared default when the helper gives no `PAM_*` text on failure.
pub fn default_failure_message() -> &'static str {
    "Authentication failed. Try again."
}
