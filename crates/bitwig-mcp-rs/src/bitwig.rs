//! The UDP link to Bitwig: sends OSC commands and keeps the [`State`] cache fresh.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rosc::OscType;
use tokio::net::{UdpSocket, lookup_host};
use tokio::task::JoinHandle;

use crate::config::Config;
use crate::protocol::{self, Scale};
use crate::state::State;

/// Handle to a running Bitwig connection.
#[derive(Debug)]
pub struct Bitwig {
    scale: Scale,
    bank_size: u8,
    socket: UdpSocket,
    state: Arc<Mutex<State>>,
    listen_addr: Option<SocketAddr>,
    listener: Option<JoinHandle<()>>,
}

impl Bitwig {
    /// Opens the send socket and starts the listener.
    ///
    /// A listen port that is already taken is not fatal: commands still work, only the
    /// state readback is missing ([`Bitwig::listening`] is then false).
    ///
    /// # Errors
    /// If the host cannot be resolved or the send socket cannot be created.
    pub async fn connect(cfg: &Config) -> Result<Self> {
        let target = lookup_host((cfg.host.as_str(), cfg.send_port))
            .await
            .with_context(|| format!("cannot resolve host {:?}", cfg.host))?
            .next()
            .with_context(|| format!("host {:?} has no address", cfg.host))?;
        let local = if target.is_ipv4() {
            "0.0.0.0:0"
        } else {
            "[::]:0"
        };
        let socket = UdpSocket::bind(local)
            .await
            .context("cannot open the send socket")?;
        socket
            .connect(target)
            .await
            .with_context(|| format!("cannot target {target}"))?;

        let state = Arc::new(Mutex::new(State::default()));
        let (listen_addr, listener) = match UdpSocket::bind(("0.0.0.0", cfg.listen_port)).await {
            Ok(sock) => {
                let addr = sock.local_addr().ok();
                (addr, Some(tokio::spawn(listen(sock, Arc::clone(&state)))))
            }
            Err(e) => {
                tracing::warn!(
                    port = cfg.listen_port,
                    "cannot listen for Bitwig state ({e}); is another instance running? \
                     Commands still work, state readback does not"
                );
                (None, None)
            }
        };
        Ok(Self {
            scale: Scale::new(cfg.resolution),
            bank_size: cfg.bank_size,
            socket,
            state,
            listen_addr,
            listener,
        })
    }

    /// Value scaling in use.
    #[must_use]
    pub fn scale(&self) -> Scale {
        self.scale
    }

    /// Bank page size in use.
    #[must_use]
    pub fn bank_size(&self) -> u8 {
        self.bank_size
    }

    /// True when the state listener runs.
    #[must_use]
    pub fn listening(&self) -> bool {
        self.listener.is_some()
    }

    /// Address the listener is bound to.
    #[must_use]
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.listen_addr
    }

    /// Sends one OSC command.
    ///
    /// # Errors
    /// For an invalid address or when the datagram cannot be sent.
    pub async fn send(&self, addr: &str, args: Vec<OscType>) -> Result<()> {
        if let Err(e) = protocol::check_address(addr) {
            bail!(e);
        }
        let bytes = protocol::encode(addr, args)?;
        tracing::debug!(addr, "send");
        self.socket
            .send(&bytes)
            .await
            .with_context(|| format!("cannot send {addr} to Bitwig"))?;
        Ok(())
    }

    /// Asks Bitwig to resend its whole state.
    ///
    /// # Errors
    /// If the command cannot be sent.
    pub async fn refresh(&self) -> Result<()> {
        self.send("/refresh", Vec::new()).await
    }

    /// Runs `f` on the current state.
    pub fn with_state<T>(&self, f: impl FnOnce(&State) -> T) -> T {
        let guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        f(&guard)
    }

    /// Waits until `pred` holds for the state, up to `timeout`. Returns whether it did.
    pub async fn wait_for(&self, timeout: Duration, pred: impl Fn(&State) -> bool) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.with_state(&pred) {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl Drop for Bitwig {
    fn drop(&mut self) {
        if let Some(h) = &self.listener {
            h.abort();
        }
    }
}

async fn listen(sock: UdpSocket, state: Arc<Mutex<State>>) {
    let mut buf = vec![0u8; 65_535];
    loop {
        match sock.recv_from(&mut buf).await {
            Ok((n, _)) => {
                let msgs = protocol::decode(&buf[..n]);
                if msgs.is_empty() {
                    continue;
                }
                let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
                for m in &msgs {
                    guard.apply(m);
                }
            }
            Err(e) => {
                tracing::warn!("state listener stopped: {e}");
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake Bitwig: a socket we can read commands from and send state with.
    async fn fake() -> (UdpSocket, Config) {
        let sock = UdpSocket::bind("127.0.0.1:0").await.expect("bind");
        let cfg = Config {
            send_port: sock.local_addr().expect("addr").port(),
            listen_port: 0,
            ..Config::default()
        };
        (sock, cfg)
    }

    #[tokio::test]
    async fn sends_encoded_commands() {
        let (fake, cfg) = fake().await;
        let bw = Bitwig::connect(&cfg).await.expect("connect");
        bw.send("/play", vec![OscType::Int(1)]).await.expect("send");
        let mut buf = [0u8; 512];
        let (n, _) = tokio::time::timeout(Duration::from_secs(2), fake.recv_from(&mut buf))
            .await
            .expect("timeout")
            .expect("recv");
        let msgs = protocol::decode(&buf[..n]);
        assert_eq!(msgs[0].addr, "/play");
        assert_eq!(msgs[0].args, vec![OscType::Int(1)]);
    }

    #[tokio::test]
    async fn rejects_bad_address_without_sending() {
        let (_fake, cfg) = fake().await;
        let bw = Bitwig::connect(&cfg).await.expect("connect");
        assert!(bw.send("play", Vec::new()).await.is_err());
    }

    #[tokio::test]
    async fn listener_updates_state() {
        let (fake, cfg) = fake().await;
        let bw = Bitwig::connect(&cfg).await.expect("connect");
        assert!(bw.listening());
        let to = bw.listen_addr().expect("listen addr");
        let bytes = protocol::encode("/track/1/name", vec![OscType::String("Lead".into())])
            .expect("encode");
        fake.send_to(&bytes, ("127.0.0.1", to.port()))
            .await
            .expect("send");
        let ok = bw
            .wait_for(Duration::from_secs(2), |s| s.get("/track/1/name").is_some())
            .await;
        assert!(ok, "state never updated");
    }

    #[tokio::test]
    async fn taken_listen_port_is_not_fatal() {
        let (_fake, mut cfg) = fake().await;
        let blocker = UdpSocket::bind("0.0.0.0:0").await.expect("bind");
        cfg.listen_port = blocker.local_addr().expect("addr").port();
        let bw = Bitwig::connect(&cfg).await.expect("connect");
        assert!(!bw.listening());
    }

    #[tokio::test]
    async fn wait_for_times_out() {
        let (_fake, cfg) = fake().await;
        let bw = Bitwig::connect(&cfg).await.expect("connect");
        assert!(
            !bw.wait_for(Duration::from_millis(60), |s| !s.is_empty())
                .await
        );
    }
}
