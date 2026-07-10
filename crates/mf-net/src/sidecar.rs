//! `SidecarProcess` — locates and spawns the TypeScript sim sidecar, parses
//! its one-line stdout handshake, and kills it on drop (spec §3.2, §2.3).

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct HandshakeLine {
    mf: String,
    #[serde(rename = "protocolVersion")]
    protocol_version: u32,
    port: u16,
    #[allow(dead_code)]
    pid: u32,
}

/// How long to wait for the sidecar's stdout handshake line before giving up.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);

pub struct SidecarProcess {
    child: Child,
    pub port: u16,
}

impl SidecarProcess {
    /// Lookup order (spec §3.2 `sidecar.rs`):
    /// 1. `$MF_SIDECAR_PATH` (exact binary path)
    /// 2. a `metroforge-sidecar[.exe]` next to the running exe
    /// 3. dev fallback: `bun run sidecar/index.ts` with cwd `/root/metroforge`
    ///
    /// `headless_speed`, if set, is passed as `--headless-speed <n>`.
    pub fn spawn(headless_speed: Option<f64>) -> anyhow::Result<Self> {
        let (mut cmd, launch_desc) = Self::resolve_launch()?;
        cmd.arg("--port").arg("0");
        if let Some(speed) = headless_speed {
            cmd.arg("--headless-speed").arg(speed.to_string());
        }
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .stdin(Stdio::null());

        // The Bun-compiled sidecar is a console-subsystem exe; without this
        // flag, launching the game on Windows pops a second empty console
        // window next to the game window.
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn sidecar ({launch_desc}): {e}"))?;

        let stdout = child.stdout.take().expect("piped stdout");
        let handshake = read_handshake(stdout)?;
        if handshake.mf != "sidecar" {
            anyhow::bail!("unexpected handshake line (mf={:?})", handshake.mf);
        }
        if handshake.protocol_version != mf_protocol::PROTOCOL_VERSION {
            anyhow::bail!(
                "sidecar protocol version {} != client {}",
                handshake.protocol_version,
                mf_protocol::PROTOCOL_VERSION
            );
        }

        Ok(SidecarProcess {
            child,
            port: handshake.port,
        })
    }

    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    fn resolve_launch() -> anyhow::Result<(Command, String)> {
        if let Ok(path) = std::env::var("MF_SIDECAR_PATH") {
            let path = PathBuf::from(path);
            if path.is_file() {
                let desc = format!("$MF_SIDECAR_PATH={}", path.display());
                return Ok((Command::new(path), desc));
            }
            tracing::warn!(
                "MF_SIDECAR_PATH={} set but not a file; falling back",
                path.display()
            );
        }

        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join(sidecar_binary_name());
                if candidate.is_file() {
                    let desc = format!("next to exe: {}", candidate.display());
                    return Ok((Command::new(candidate), desc));
                }
            }
        }

        // Dev fallback: run the sidecar's TS entrypoint directly under bun,
        // from the sibling `metroforge` checkout.
        let bun = locate_bun();
        let metroforge_dir = PathBuf::from("/root/metroforge");
        if !metroforge_dir.join("sidecar").join("index.ts").is_file() {
            anyhow::bail!(
                "no sidecar binary found (checked $MF_SIDECAR_PATH, next-to-exe) and dev fallback \
                 {}/sidecar/index.ts does not exist yet",
                metroforge_dir.display()
            );
        }
        let mut cmd = Command::new(&bun);
        cmd.current_dir(&metroforge_dir)
            .arg("run")
            .arg("sidecar/index.ts");
        let desc = format!(
            "dev fallback: {} run sidecar/index.ts (cwd {})",
            bun.display(),
            metroforge_dir.display()
        );
        Ok((cmd, desc))
    }
}

impl Drop for SidecarProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "windows")]
fn sidecar_binary_name() -> &'static str {
    "metroforge-sidecar.exe"
}

#[cfg(not(target_os = "windows"))]
fn sidecar_binary_name() -> &'static str {
    "metroforge-sidecar"
}

fn locate_bun() -> PathBuf {
    // Prefer `bun` on PATH; fall back to the well-known install location used
    // on this box (`~/.bun/bin/bun`).
    if let Ok(path) = which_on_path("bun") {
        return path;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let candidate = PathBuf::from(home).join(".bun/bin/bun");
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from("bun")
}

fn which_on_path(bin: &str) -> anyhow::Result<PathBuf> {
    let path_var = std::env::var_os("PATH").ok_or_else(|| anyhow::anyhow!("no PATH"))?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    anyhow::bail!("{bin} not found on PATH")
}

fn read_handshake(stdout: std::process::ChildStdout) -> anyhow::Result<HandshakeLine> {
    let start = Instant::now();
    // A single blocking read_line is fine here: Boot runs this as a one-shot
    // startup step, and the sidecar is expected to print its handshake
    // within HANDSHAKE_TIMEOUT. We still guard against a hang by racing a
    // background thread against a timeout channel.
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut l = String::new();
        let res = reader.read_line(&mut l).map(|_| l);
        let _ = tx.send(res);
    });
    let line = rx.recv_timeout(HANDSHAKE_TIMEOUT).map_err(|_| {
        anyhow::anyhow!(
            "timed out waiting {:?} for sidecar handshake",
            start.elapsed()
        )
    })??;
    let trimmed = line.trim();
    if trimmed.is_empty() {
        anyhow::bail!("sidecar exited before printing a handshake line");
    }
    serde_json::from_str(trimmed)
        .map_err(|e| anyhow::anyhow!("bad handshake line {trimmed:?}: {e}"))
}
