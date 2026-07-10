//! `WsTransport` — the concrete `SimTransport` used by the desktop client: a
//! single background thread owns a blocking `tungstenite` WebSocket, reading
//! with a short timeout so it can interleave draining the outbound queue.
//! Two crossbeam channels bridge it to the Bevy ECS (spec §3.2).

use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, Receiver, Sender, TryRecvError};
use mf_protocol::envelope::FromSimJson;
use mf_protocol::{decode_binary, FromSimMsg, ToSim};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use crate::transport::SimTransport;

/// How long with zero inbound traffic before the transport calls itself dead
/// (spec §1.4: "No inbound traffic for 10 s -> client declares sim dead").
const LIVENESS_WINDOW: Duration = Duration::from_secs(10);
/// Read-timeout granularity for the worker loop; also the max latency before
/// a freshly queued outbound message gets flushed.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct WsTransport {
    outbound_tx: Sender<ToSim>,
    inbound_rx: Receiver<FromSimMsg>,
    last_inbound_millis: Arc<AtomicU64>,
    started_at: Instant,
    closed: Arc<AtomicBool>,
    _worker: JoinHandle<()>,
}

impl WsTransport {
    /// Connect to `ws_url` (e.g. `ws://127.0.0.1:PORT`) and spawn the worker
    /// thread. Blocks until the initial TCP+WS handshake completes or fails.
    pub fn connect(ws_url: &str) -> anyhow::Result<Self> {
        let (mut socket, _response) = tungstenite::connect(ws_url)?;
        set_read_timeout(&mut socket, Some(POLL_INTERVAL));

        let (outbound_tx, outbound_rx) = unbounded::<ToSim>();
        let (inbound_tx, inbound_rx) = unbounded::<FromSimMsg>();
        let last_inbound_millis = Arc::new(AtomicU64::new(0));
        let closed = Arc::new(AtomicBool::new(false));
        let started_at = Instant::now();

        let worker_last_inbound = last_inbound_millis.clone();
        let worker_closed = closed.clone();
        let worker = std::thread::Builder::new()
            .name("mf-net-ws".into())
            .spawn(move || {
                run_worker(
                    socket,
                    outbound_rx,
                    inbound_tx,
                    worker_last_inbound,
                    started_at,
                    worker_closed,
                );
            })
            .expect("failed to spawn mf-net-ws thread");

        Ok(WsTransport {
            outbound_tx,
            inbound_rx,
            last_inbound_millis,
            started_at,
            closed,
            _worker: worker,
        })
    }
}

impl SimTransport for WsTransport {
    fn send(&self, msg: ToSim) -> anyhow::Result<()> {
        self.outbound_tx
            .send(msg)
            .map_err(|e| anyhow::anyhow!("mf-net worker gone: {e}"))
    }

    fn try_recv(&self) -> Option<FromSimMsg> {
        match self.inbound_rx.try_recv() {
            Ok(msg) => Some(msg),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => None,
        }
    }

    fn is_alive(&self) -> bool {
        if self.closed.load(Ordering::Relaxed) {
            return false;
        }
        let last = self.last_inbound_millis.load(Ordering::Relaxed);
        if last == 0 {
            // Nothing received yet; alive as long as we're still inside the
            // liveness window since connect (hello should arrive immediately).
            return self.started_at.elapsed() < LIVENESS_WINDOW;
        }
        let elapsed_ms = self.started_at.elapsed().as_millis() as u64;
        elapsed_ms.saturating_sub(last) < LIVENESS_WINDOW.as_millis() as u64
    }
}

fn set_read_timeout(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>, timeout: Option<Duration>) {
    if let MaybeTlsStream::Plain(tcp) = socket.get_ref() {
        let _ = tcp.set_read_timeout(timeout);
    }
}

fn run_worker(
    mut socket: WebSocket<MaybeTlsStream<TcpStream>>,
    outbound_rx: Receiver<ToSim>,
    inbound_tx: Sender<FromSimMsg>,
    last_inbound_millis: Arc<AtomicU64>,
    started_at: Instant,
    closed: Arc<AtomicBool>,
) {
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                mark_alive(&last_inbound_millis, started_at);
                match serde_json::from_str::<mf_protocol::Envelope>(&text) {
                    Ok(env) => match FromSimJson::from_envelope(env) {
                        Ok(json_msg) => {
                            if inbound_tx.send(FromSimMsg::from(json_msg)).is_err() {
                                break;
                            }
                        }
                        Err(e) => tracing::warn!("mf-net: bad envelope: {e}"),
                    },
                    Err(e) => tracing::warn!("mf-net: malformed JSON frame: {e}"),
                }
            }
            Ok(Message::Binary(bytes)) => {
                mark_alive(&last_inbound_millis, started_at);
                match decode_binary(&bytes) {
                    Ok(bin_msg) => {
                        if inbound_tx.send(FromSimMsg::from(bin_msg)).is_err() {
                            break;
                        }
                    }
                    Err(e) => tracing::warn!("mf-net: bad binary frame: {e}"),
                }
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {
                mark_alive(&last_inbound_millis, started_at);
            }
            Ok(Message::Close(_)) => {
                tracing::info!("mf-net: sim closed the connection");
                break;
            }
            Ok(Message::Frame(_)) => {}
            Err(tungstenite::Error::Io(ref e))
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                // No message within POLL_INTERVAL; fall through to flush outbound.
            }
            Err(e) => {
                tracing::warn!("mf-net: read error, closing: {e}");
                break;
            }
        }

        // Drain everything currently queued for send.
        loop {
            match outbound_rx.try_recv() {
                Ok(msg) => {
                    let env = msg.to_envelope();
                    let is_shutdown = matches!(msg, ToSim::Shutdown);
                    let text = match serde_json::to_string(&env) {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::warn!("mf-net: failed to encode outbound envelope: {e}");
                            continue;
                        }
                    };
                    if let Err(e) = socket.send(Message::Text(text)) {
                        tracing::warn!("mf-net: send error: {e}");
                        closed.store(true, Ordering::Relaxed);
                        return;
                    }
                    if is_shutdown {
                        let _ = socket.close(None);
                        closed.store(true, Ordering::Relaxed);
                        return;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Owning WsTransport (and SimLink) dropped; shut down.
                    let _ = socket.close(None);
                    closed.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
    }
    closed.store(true, Ordering::Relaxed);
}

fn mark_alive(last_inbound_millis: &Arc<AtomicU64>, started_at: Instant) {
    let now_ms = started_at.elapsed().as_millis() as u64;
    last_inbound_millis.store(now_ms, Ordering::Relaxed);
}
