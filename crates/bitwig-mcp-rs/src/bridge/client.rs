//! TCP client for the bridge extension: request/reply with ids, lazy (re)connect.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value as Json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::net::tcp::OwnedWriteHalf;
use tokio::sync::{Mutex, oneshot};

use super::protocol::{self, Command, Reply, Request};

type Pending = Arc<std::sync::Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

#[derive(Debug)]
struct Conn {
    writer: OwnedWriteHalf,
    reader: tokio::task::JoinHandle<()>,
    /// Set by the reader when the socket closes.
    alive: Arc<std::sync::atomic::AtomicBool>,
}

/// Connection to the Bitwig extension.
#[derive(Debug)]
pub struct Bridge {
    addr: String,
    next_id: AtomicU64,
    pending: Pending,
    conn: Mutex<Option<Conn>>,
    /// The extension's `hello` reply of the current connection.
    hello: std::sync::Mutex<Option<Json>>,
}

/// Default reply timeout.
pub const TIMEOUT: Duration = Duration::from_secs(10);

impl Bridge {
    /// A client for `host:port`. Connects on first use.
    #[must_use]
    pub fn new(host: &str, port: u16) -> Self {
        let addr = if host.contains(':') {
            format!("[{host}]:{port}")
        } else {
            format!("{host}:{port}")
        };
        Self {
            addr,
            next_id: AtomicU64::new(1),
            pending: Arc::default(),
            conn: Mutex::new(None),
            hello: std::sync::Mutex::new(None),
        }
    }

    /// `host:port` of the extension.
    #[must_use]
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// The handshake reply of the live connection, if connected.
    #[must_use]
    pub fn hello(&self) -> Option<Json> {
        self.hello
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Sends a command and waits for its reply (default timeout).
    ///
    /// # Errors
    /// When the extension is unreachable, times out, or reports an error.
    pub async fn call(&self, command: Command) -> Result<Json> {
        self.call_with(command, TIMEOUT).await
    }

    /// Sends a command and waits for its reply up to `timeout`.
    ///
    /// # Errors
    /// When the extension is unreachable, times out, or reports an error.
    pub async fn call_with(&self, command: Command, timeout: Duration) -> Result<Json> {
        let reply = self.round_trip(command, timeout).await?;
        if reply.ok {
            Ok(reply.result)
        } else {
            Err(anyhow!(
                "Bitwig: {}",
                reply.error.unwrap_or_else(|| "unknown error".into())
            ))
        }
    }

    async fn round_trip(&self, command: Command, timeout: Duration) -> Result<Reply> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.lock_pending().insert(id, tx);
        let sent = self.send(&Request { id, command }).await;
        if let Err(e) = sent {
            self.lock_pending().remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(_)) => bail!("the Bitwig bridge closed the connection"),
            Err(_) => {
                self.lock_pending().remove(&id);
                bail!("the Bitwig bridge did not answer within {timeout:?}")
            }
        }
    }

    fn lock_pending(&self) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<Reply>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    async fn send(&self, req: &Request) -> Result<()> {
        let bytes = protocol::frame(&serde_json::to_vec(req)?);
        let mut guard = self.conn.lock().await;
        let fresh = match guard.as_ref() {
            Some(c) if c.alive.load(Ordering::Relaxed) => false,
            _ => {
                if let Some(old) = guard.take() {
                    old.reader.abort();
                }
                *guard = Some(self.connect().await?);
                true
            }
        };
        let conn = guard.as_mut().context("not connected")?;
        if let Err(e) = conn.writer.write_all(&bytes).await {
            conn.alive.store(false, Ordering::Relaxed);
            bail!("cannot write to the Bitwig bridge: {e}");
        }
        drop(guard);
        if fresh {
            tracing::info!(addr = %self.addr, "connected to the Bitwig bridge");
        }
        Ok(())
    }

    async fn connect(&self) -> Result<Conn> {
        let stream = tokio::time::timeout(Duration::from_secs(3), TcpStream::connect(&self.addr))
            .await
            .map_err(|_| anyhow!("timed out connecting to {}", self.addr))?
            .map_err(|e| {
                anyhow!(
                    "cannot reach the Bitwig bridge at {} ({e}). Is Bitwig running with the \
                     'bitwig-mcp-rs bridge' controller added (Settings → Controllers)?",
                    self.addr
                )
            })?;
        stream.set_nodelay(true).ok();
        let (mut read, writer) = stream.into_split();
        let alive = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let pending = Arc::clone(&self.pending);
        let reader_alive = Arc::clone(&alive);
        let reader = tokio::spawn(async move {
            let mut buf = Vec::new();
            let mut chunk = vec![0u8; 64 * 1024];
            'outer: loop {
                match read.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
                loop {
                    match protocol::take_frame(&mut buf) {
                        Ok(Some(payload)) => match serde_json::from_slice::<Reply>(
                            protocol::unwrap_payload(&payload),
                        ) {
                            Ok(reply) => {
                                let tx = pending
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                                    .remove(&reply.id);
                                if let Some(tx) = tx {
                                    let _ = tx.send(reply);
                                }
                            }
                            Err(e) => tracing::warn!("ignoring a malformed bridge frame: {e}"),
                        },
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!("bridge stream broken: {e}");
                            break 'outer;
                        }
                    }
                }
            }
            reader_alive.store(false, Ordering::Relaxed);
            // Fail everything still waiting: dropping the senders wakes the receivers.
            pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
        });
        let mut conn = Conn {
            writer,
            reader,
            alive,
        };
        // Handshake on the raw connection (the connection lock is held by the caller).
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.lock_pending().insert(id, tx);
        let hello = protocol::frame(&serde_json::to_vec(&Request {
            id,
            command: Command::Hello,
        })?);
        conn.writer.write_all(&hello).await?;
        let reply = tokio::time::timeout(Duration::from_secs(3), rx)
            .await
            .map_err(|_| anyhow!("the Bitwig bridge did not answer the handshake"))?
            .map_err(|_| anyhow!("the Bitwig bridge closed the connection during the handshake"))?;
        let version = reply.result["protocol"].as_u64().unwrap_or(0);
        if version != u64::from(protocol::PROTOCOL_VERSION) {
            conn.reader.abort();
            bail!(
                "protocol mismatch: the Bitwig extension speaks v{version}, this server v{}. \
                 Reinstall the extension from this release (`just bridge-install`)",
                protocol::PROTOCOL_VERSION
            );
        }
        *self
            .hello
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reply.result);
        Ok(conn)
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! An in-process stand-in for the Java extension, for tests.

    use super::*;
    use tokio::net::TcpListener;

    /// Starts a fake extension. `respond` maps each request to a result (Ok) or error (Err).
    /// Every request is also pushed to the returned log.
    pub(crate) async fn spawn<F>(respond: F) -> (u16, Arc<std::sync::Mutex<Vec<Json>>>)
    where
        F: Fn(&Json) -> Result<Json, String> + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let log: Arc<std::sync::Mutex<Vec<Json>>> = Arc::default();
        let log2 = Arc::clone(&log);
        let respond = Arc::new(respond);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let log = Arc::clone(&log2);
                let respond = Arc::clone(&respond);
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = vec![0u8; 4096];
                    loop {
                        let Ok(n) = sock.read(&mut chunk).await else {
                            return;
                        };
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        while let Ok(Some(p)) = protocol::take_frame(&mut buf) {
                            let req: Json = serde_json::from_slice(&p).expect("json");
                            let id = req["id"].clone();
                            let reply = if req["cmd"] == "hello" {
                                serde_json::json!({"id": id, "ok": true, "result": {
                                    "protocol": protocol::PROTOCOL_VERSION,
                                    "extension_version": "test", "api_version": 25}})
                            } else {
                                log.lock().expect("log").push(req.clone());
                                match respond(&req) {
                                    Ok(r) => serde_json::json!({"id": id, "ok": true, "result": r}),
                                    Err(e) => {
                                        serde_json::json!({"id": id, "ok": false, "error": e})
                                    }
                                }
                            };
                            let bytes = protocol::frame(&serde_json::to_vec(&reply).expect("json"));
                            if sock.write_all(&bytes).await.is_err() {
                                return;
                            }
                        }
                    }
                });
            }
        });
        (port, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::protocol::TransportAction;

    #[tokio::test]
    async fn round_trip_with_handshake() {
        let (port, log) = fake::spawn(|req| Ok(serde_json::json!({"echo": req["cmd"]}))).await;
        let b = Bridge::new("127.0.0.1", port);
        let r = b
            .call(Command::Transport {
                action: TransportAction::Play,
            })
            .await
            .expect("call");
        assert_eq!(r["echo"], "transport");
        assert_eq!(b.hello().expect("hello")["extension_version"], "test");
        assert_eq!(log.lock().expect("log")[0]["action"], "play");
    }

    #[tokio::test]
    async fn errors_from_bitwig_are_errors() {
        let (port, _) = fake::spawn(|_| Err("no such track".into())).await;
        let b = Bridge::new("127.0.0.1", port);
        let e = b.call(Command::GetProject).await.expect_err("error");
        assert!(e.to_string().contains("no such track"), "{e}");
    }

    #[tokio::test]
    async fn unreachable_bridge_explains_the_setup() {
        // Bind and drop to get a port that is very likely closed.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            l.local_addr().expect("addr").port()
        };
        let b = Bridge::new("127.0.0.1", port);
        let e = b.call(Command::GetProject).await.expect_err("down");
        assert!(e.to_string().contains("Settings"), "{e}");
    }

    #[tokio::test]
    async fn concurrent_calls_get_their_own_replies() {
        let (port, _) = fake::spawn(|req| Ok(req["query"].clone())).await;
        let b = Arc::new(Bridge::new("127.0.0.1", port));
        let mut tasks = Vec::new();
        for i in 0..20 {
            let b = Arc::clone(&b);
            tasks.push(tokio::spawn(async move {
                let r = b
                    .call(Command::ListActions {
                        query: i.to_string(),
                    })
                    .await
                    .expect("call");
                assert_eq!(r, Json::String(i.to_string()));
            }));
        }
        for t in tasks {
            t.await.expect("task");
        }
    }
}
