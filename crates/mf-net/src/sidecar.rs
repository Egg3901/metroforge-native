//! Legacy sidecar compatibility shims after embedded cutover.
//!
//! The Bun sidecar process manager is removed. We keep the reconnect/fatal
//! diagnostics enums and a tiny `SidecarProcess` placeholder so existing UI
//! code compiles while no longer spawning external processes.

use std::process::ExitStatus;

/// Why transport/reconnect considered the old sidecar path dead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarDeathReason {
    /// Child process exited (possibly with code).
    ProcessExited { code: Option<i32> },
    /// Transport has been silent beyond the liveness threshold.
    WebsocketSilence { silence_ms: u64 },
}

impl SidecarDeathReason {
    /// Short label for HUD diagnostics.
    pub fn label(&self) -> &'static str {
        match self {
            SidecarDeathReason::ProcessExited { .. } => "process exited",
            SidecarDeathReason::WebsocketSilence { .. } => "transport silence",
        }
    }

    /// Human-friendly detail line.
    pub fn detail(&self) -> String {
        match self {
            SidecarDeathReason::ProcessExited { code } => match code {
                Some(c) => format!("process exited with code {c}"),
                None => "process exited without a code".to_string(),
            },
            SidecarDeathReason::WebsocketSilence { silence_ms } => {
                format!("no transport traffic for {silence_ms}ms")
            }
        }
    }
}

/// Placeholder kept so `SimLink` shape stays stable for reconnect/HUD code.
#[derive(Debug, Default)]
pub struct SidecarProcess;

impl SidecarProcess {
    /// Sidecar spawning is removed after cutover.
    pub fn spawn(_headless_speed: Option<f64>) -> anyhow::Result<Self> {
        Err(anyhow::anyhow!(
            "external sidecar is removed; use embedded transport"
        ))
    }

    /// Legacy shim for callers that still request a sidecar URL.
    pub fn ws_url(&self) -> String {
        "ws://127.0.0.1:0".to_string()
    }

    /// No external process exists post-cutover.
    pub fn try_exit_status(&mut self) -> Option<ExitStatus> {
        None
    }

    /// No-op: no sidecar stderr ring exists anymore.
    pub fn log_tail(&self) -> String {
        String::new()
    }

    /// No-op kill for removed sidecar process.
    pub fn kill_now(&mut self) {}
}
