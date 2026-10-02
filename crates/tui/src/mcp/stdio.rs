//! MCP newline framing over the broker-owned reviewed stdio process.
use anyhow::Result;
use tokio::process::ChildStdout;

use super::process_broker::BrokerSession;
use super::wire::MAX_MCP_RESPONSE_BYTES;
use super::{McpServerConfig, McpTransport};

pub(super) struct StdioTransport {
    pub(super) session: BrokerSession,
    reader: tokio::io::BufReader<ChildStdout>,
    /// Partial frame bytes survive cancellation of the receive future.
    pub(super) pending_line: Vec<u8>,
}

impl StdioTransport {
    pub(super) fn spawn(
        server_name: &str,
        command: &str,
        config: &McpServerConfig,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<Self> {
        let (session, stdout) = BrokerSession::spawn(server_name, command, config, cancel_token)?;
        Ok(Self {
            session,
            reader: tokio::io::BufReader::new(stdout),
            pending_line: Vec::new(),
        })
    }
}

#[async_trait::async_trait]
impl McpTransport for StdioTransport {
    async fn last_stderr_line(&self) -> Option<String> {
        self.session.last_stderr_line().await
    }

    async fn send(&mut self, mut msg: Vec<u8>) -> Result<()> {
        msg.push(b'\n');
        self.session.write(&msg).await
    }

    /// Non-blocking liveness probe: a reaped child means the transport is
    /// dead even though the `Ready` flag is still set (#6187). The sync
    /// trait contract forbids awaiting the lock, so a contended lock reads
    /// as alive — the read side observes the death on the next call.
    fn probe_dead(&self) -> bool {
        self.session.probe_dead()
    }

    async fn recv(&mut self) -> Result<Vec<u8>> {
        loop {
            // Bounded read: a server emitting a newline-free multi-GB "line"
            // must not OOM us (read_line is unbounded).
            let bytes = match read_line_capped(
                &mut self.reader,
                &mut self.pending_line,
                MAX_MCP_RESPONSE_BYTES,
            )
            .await
            {
                Ok(b) => b,
                Err(err) => {
                    if let Some(stderr) = self.session.stderr_context().await {
                        anyhow::bail!("Stdio transport read error: {err}\n{stderr}");
                    }
                    return Err(err.into());
                }
            };
            if bytes == 0 {
                // Let the stderr drain task catch up before snapshotting, and
                // name the exit status: a reviewed plugin's stderr is never
                // retained, so the status is the only reason the operator
                // gets when the child dies before the handshake (#5916).
                tokio::task::yield_now().await;
                let exit = self.session.exit_status().await;
                let exit = exit.map_or_else(String::new, |status| format!(" ({status})"));
                if let Some(stderr) = self.session.stderr_context().await {
                    anyhow::bail!("Stdio transport closed{exit}\n{stderr}");
                }
                anyhow::bail!("Stdio transport closed{exit}");
            }

            let line_bytes = std::mem::take(&mut self.pending_line);
            let line = String::from_utf8_lossy(&line_bytes);
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            return Ok(trimmed.as_bytes().to_vec());
        }
    }

    /// Send SIGTERM and wait up to `STDIO_SHUTDOWN_GRACE` for graceful exit,
    /// then force termination and reap the child as the backstop.
    async fn shutdown(&mut self) {
        self.session.shutdown().await;
    }
}

/// Continue one newline-terminated line in caller-owned `out`, aborting if it
/// exceeds `max` bytes. Cancellation retains consumed bytes; the caller clears
/// the buffer only after receiving a complete frame. Returns the total bytes
/// accumulated; 0 means EOF.
pub(crate) async fn read_line_capped<R>(
    reader: &mut R,
    out: &mut Vec<u8>,
    max: usize,
) -> std::io::Result<usize>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;
    loop {
        let (chunk, consumed, done) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                (Vec::new(), 0usize, true)
            } else if let Some(pos) = available.iter().position(|&b| b == b'\n') {
                (available[..=pos].to_vec(), pos + 1, true)
            } else {
                (available.to_vec(), available.len(), false)
            }
        };
        if consumed > 0 {
            reader.consume(consumed);
        }
        out.extend_from_slice(&chunk);
        if out.len() > max {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("MCP stdio line exceeded {max} bytes"),
            ));
        }
        if done {
            break;
        }
    }
    Ok(out.len())
}

#[cfg(test)]
mod read_cap_tests {
    use super::read_line_capped;

    #[tokio::test]
    async fn cancelled_partial_read_preserves_next_frame() {
        use futures_util::FutureExt;
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(4096);
        let mut reader = tokio::io::BufReader::new(reader);
        let prefix = br#"{"jsonrpc":"2.0","id":"1","result":"#;
        writer.write_all(prefix).await.unwrap();
        let mut pending = Vec::new();
        // Poll through the consumed prefix to Pending, then drop the future.
        assert!(
            read_line_capped(&mut reader, &mut pending, 1024)
                .now_or_never()
                .is_none()
        );
        assert_eq!(pending, prefix);
        writer.write_all(b"null}\n").await.unwrap();
        read_line_capped(&mut reader, &mut pending, 1024)
            .await
            .unwrap();
        let first: serde_json::Value =
            serde_json::from_slice(&std::mem::take(&mut pending)).unwrap();
        assert_eq!(first["id"], "1");
        writer
            .write_all(b"{\"id\":\"2\",\"result\":true}\n")
            .await
            .unwrap();
        read_line_capped(&mut reader, &mut pending, 1024)
            .await
            .unwrap();
        let second: serde_json::Value = serde_json::from_slice(&pending).unwrap();
        assert_eq!(second["id"], "2");
        assert_eq!(second["result"], true);
    }

    #[tokio::test]
    async fn resumed_frame_still_enforces_cap_at_newline() {
        use futures_util::FutureExt;
        use tokio::io::AsyncWriteExt;
        let (mut writer, reader) = tokio::io::duplex(4096);
        let mut reader = tokio::io::BufReader::new(reader);
        let mut pending = Vec::new();
        writer.write_all(b"1234").await.unwrap();
        assert!(
            read_line_capped(&mut reader, &mut pending, 6)
                .now_or_never()
                .is_none()
        );
        writer.write_all(b"567\n").await.unwrap();
        assert_eq!(
            read_line_capped(&mut reader, &mut pending, 6)
                .await
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }

    #[tokio::test]
    async fn reads_a_line_and_reports_eof() {
        let data = b"hello\nworld\n".to_vec();
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(data));
        let mut out = Vec::new();
        assert_eq!(
            read_line_capped(&mut reader, &mut out, 1024).await.unwrap(),
            6
        );
        assert_eq!(out, b"hello\n");
        out.clear();
        assert_eq!(
            read_line_capped(&mut reader, &mut out, 1024).await.unwrap(),
            6
        );
        assert_eq!(out, b"world\n");
        out.clear();
        // EOF.
        assert_eq!(
            read_line_capped(&mut reader, &mut out, 1024).await.unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn aborts_on_newline_free_line_over_cap() {
        let data = vec![b'x'; 4096]; // no newline
        let mut reader = tokio::io::BufReader::new(std::io::Cursor::new(data));
        let mut out = Vec::new();
        let err = read_line_capped(&mut reader, &mut out, 1024)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }
}
