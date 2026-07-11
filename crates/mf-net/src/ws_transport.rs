//! `WsTransport` — the concrete `SimTransport` used by the desktop client: a
//! single background thread owns a blocking `tungstenite` WebSocket, reading
//! with a short timeout so it can interleave draining the outbound queue.
//! Two crossbeam channels bridge it to the Bevy ECS (spec §3.2).
//!
//! Outbound latency: a queued send can only be picked up once `socket.read()`
//! returns (either a real inbound frame arrived, or the read timed out), so
//! `POLL_INTERVAL` is also the worst-case delay before a player command (e.g.
//! click-to-build) reaches the wire. We chose "shrink the poll interval"
//! (simplest fix) over restructuring to a non-blocking writer path: sync
//! tungstenite has no socket split, so a separate writer would need a second
//! `WebSocket` over a cloned `TcpStream`, which is fragile with WS framing
//! and not worth it when 4ms already meets the latency bar. See
//! `POLL_INTERVAL` below for the idle-cost accounting.

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
/// (1.0: websocket silence > 5 s). Measured against `last_inbound_millis`
/// (wall-clock, set only on actual inbound frames/pings/pongs — see
/// `mark_alive`), so it's independent of `POLL_INTERVAL`. `plugin.rs` pings
/// every 2.5 s (half this window) so one dropped pong still leaves slack
/// before `is_alive()` would flap.
pub const LIVENESS_WINDOW: Duration = Duration::from_secs(5);
/// Read-timeout granularity for the worker loop: on every timeout (no inbound
/// frame arrived within the window) the loop falls through to drain the
/// outbound queue, so this both bounds worst-case outbound-send latency and
/// sets the idle wake-up rate.
///
/// Kept at 4ms (down from an earlier 50ms) so a queued outbound command —
/// e.g. a click-to-build — never waits more than ~4ms behind a blocking read
/// timeout before being written to the socket, comfortably under the ~5ms
/// target. Cost on a fully idle connection (nothing queued, nothing
/// incoming): the worker wakes ~250x/s, and each wake is one `read()` syscall
/// that immediately returns `WouldBlock`/`TimedOut` plus one empty
/// `try_recv()` — no allocation, no wire traffic. That's negligible CPU for a
/// desktop game client: a single OS thread parked in a short blocking read
/// (not a busy-spin), waking at a rate far below what the render/sim threads
/// already cost per frame at 60fps.
const POLL_INTERVAL: Duration = Duration::from_millis(4);

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
        self.silence_duration() < LIVENESS_WINDOW
    }

    fn silence_duration(&self) -> Duration {
        let last = self.last_inbound_millis.load(Ordering::Relaxed);
        if last == 0 {
            // Nothing received yet; measure silence from connect time
            // (hello should arrive immediately).
            return self.started_at.elapsed();
        }
        let elapsed_ms = self.started_at.elapsed().as_millis() as u64;
        Duration::from_millis(elapsed_ms.saturating_sub(last))
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
                // No inbound message within POLL_INTERVAL; fall through to
                // drain+send outbound (this is the common case at 4ms — it's
                // what bounds outbound latency, not an error path).
            }
            Err(e) => {
                tracing::warn!("mf-net: read error, closing: {e}");
                break;
            }
        }

        // Drain everything currently queued for send. `WebSocket::send`
        // is `write` + `flush` in one call, so each message reaches the
        // socket immediately rather than sitting in an internal write
        // buffer for the next batch — no explicit `flush()` needed here.
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
                    // tungstenite 0.26+: `Message::Text` holds `Utf8Bytes`;
                    // `Message::text` accepts anything `Into<Utf8Bytes>`
                    // (including `String`) so the wire payload is unchanged.
                    if let Err(e) = socket.send(Message::text(text)) {
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
