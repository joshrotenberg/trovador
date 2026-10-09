mod state;

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout},
};

const RPC_TIMEOUT: Duration = Duration::from_secs(65);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

/// Sequential MCP client used only by integration tests against the real server.
pub struct Client {
    pub initialization: Value,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    log_path: PathBuf,
    _log_dir: TempDir,
    finished: bool,
}

impl Client {
    pub async fn start(binary: impl AsRef<Path>) -> Result<Self, String> {
        let log_dir = tempfile::tempdir().map_err(|e| format!("create log directory: {e}"))?;
        let log_path = log_dir.path().join("server.log");
        let log = File::create(&log_path).map_err(|e| format!("create server log: {e}"))?;
        let mut child = state::server_command(binary.as_ref(), log_dir.path())
            .arg("--stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| format!("start {}: {e}", binary.as_ref().display()))?;
        let mut client = Self {
            initialization: Value::Null,
            stdin: child.stdin.take(),
            stdout: BufReader::new(child.stdout.take().expect("stdout was piped")).lines(),
            child,
            next_id: 0,
            log_path,
            _log_dir: log_dir,
            finished: false,
        };
        let initialize = async {
            client.initialization = client
                .rpc(
                    "initialize",
                    json!({
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "clientInfo": {"name": "tosi-live-test", "version": "1"}
                    }),
                )
                .await?;
            tokio::time::timeout(
                RPC_TIMEOUT,
                client.write(json!({
                    "jsonrpc": "2.0", "method": "notifications/initialized"
                })),
            )
            .await
            .map_err(|_| "timed out sending initialized notification".to_owned())?
        }
        .await;
        if let Err(error) = initialize {
            let cleanup = client.finish().await;
            return Err(match cleanup {
                Ok(()) => client.diagnostic(error),
                Err(cleanup) => format!("{error}\nInitialization cleanup: {cleanup}"),
            });
        }
        Ok(client)
    }

    pub async fn rpc(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let result = tokio::time::timeout(RPC_TIMEOUT, self.rpc_inner(method, params)).await;
        match result {
            Ok(result) => result.map_err(|error| self.diagnostic(error)),
            Err(_) => Err(self.diagnostic(format!("{method} exceeded the 65-second RPC timeout"))),
        }
    }

    async fn rpc_inner(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .await?;
        loop {
            let line = self
                .stdout
                .next_line()
                .await
                .map_err(|e| format!("read {method} response: {e}"))?
                .ok_or_else(|| format!("server closed stdout before answering {method}"))?;
            let response: Value = serde_json::from_str(&line)
                .map_err(|e| format!("invalid JSON in {method} response: {e}"))?;
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(format!("{method} returned a JSON-RPC error: {error}"));
            }
            return response
                .get("result")
                .cloned()
                .ok_or_else(|| format!("{method} response contained no result"));
        }
    }

    pub async fn tool(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let result = self
            .rpc("tools/call", json!({"name": name, "arguments": args}))
            .await?;
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            return Err(self.diagnostic(format!("tool {name} failed: {result}")));
        }
        if let Some(value) = result.get("structuredContent") {
            return Ok(value.clone());
        }
        let text = result
            .get("content")
            .and_then(Value::as_array)
            .and_then(|content| {
                content.iter().find_map(|item| {
                    (item.get("type").and_then(Value::as_str) == Some("text"))
                        .then(|| item.get("text").and_then(Value::as_str))
                        .flatten()
                })
            })
            .ok_or_else(|| {
                self.diagnostic(format!("tool {name} returned no JSON or text content"))
            })?;
        serde_json::from_str(text)
            .map_err(|e| self.diagnostic(format!("tool {name} returned invalid JSON text: {e}")))
    }

    async fn write(&mut self, message: Value) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("server stdin is closed")?;
        let mut line = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
        line.push(b'\n');
        stdin
            .write_all(&line)
            .await
            .map_err(|e| format!("write MCP request: {e}"))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("flush MCP request: {e}"))
    }

    /// Give the server EOF so it can cancel and reap providers before exiting.
    pub async fn finish(&mut self) -> Result<(), String> {
        if self.finished {
            return Ok(());
        }
        drop(self.stdin.take());
        // A timed-out RPC may still produce a response. Drain stdout while
        // waiting so a full pipe cannot block the server's graceful shutdown.
        let outcome = tokio::time::timeout(SHUTDOWN_TIMEOUT, async {
            let drain = async {
                while self.stdout.next_line().await?.is_some() {}
                Ok::<(), std::io::Error>(())
            };
            let (status, ()) = tokio::try_join!(self.child.wait(), drain)?;
            Ok::<_, std::io::Error>(status)
        })
        .await;
        let result = match outcome {
            Ok(Ok(status)) => {
                self.finished = true;
                if status.success() {
                    Ok(())
                } else {
                    Err(format!("server exited unsuccessfully: {status}"))
                }
            }
            Ok(Err(error)) => Err(format!("wait for server shutdown: {error}")),
            Err(_) => {
                // Only fall back to killing after allowing graceful provider
                // cleanup. Report it as failure, never as a successful stop.
                let killed = self.child.kill().await;
                self.finished = killed.is_ok();
                Err(match killed {
                    Ok(()) => {
                        "server exceeded the 15-second shutdown timeout; killed and reaped server"
                            .into()
                    }
                    Err(error) => format!(
                        "server exceeded the 15-second shutdown timeout; kill failed: {error}"
                    ),
                })
            }
        };
        result.map_err(|error| self.diagnostic(error))
    }

    fn diagnostic(&self, message: String) -> String {
        let tail = (|| -> std::io::Result<String> {
            let mut file = File::open(&self.log_path)?;
            let length = file.metadata()?.len();
            file.seek(SeekFrom::Start(length.saturating_sub(8192)))?;
            let mut bytes = Vec::new();
            file.take(8192).read_to_end(&mut bytes)?;
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        })();
        match tail {
            Ok(tail) if !tail.trim().is_empty() => {
                format!("{message}\nServer stderr tail:\n{tail}")
            }
            _ => message,
        }
    }
}
