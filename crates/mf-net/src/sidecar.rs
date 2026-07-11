//! `SidecarProcess` — locates and spawns the TypeScript sim sidecar, parses
//! its one-line stdout handshake, captures a rolling stderr log tail for
//! diagnostics, and kills it on drop (spec §3.2, §2.3).
//!
//! Orphan prevention (1.0):
//! - **Unix:** the child is placed in its own process group and
//!   `PR_SET_PDEATHSIG` is set so a client crash delivers `SIGTERM` to the
//!   sidecar (and any grandchildren in the group are killed on `Drop`).
//! - **Windows:** the child is assigned to a Job Object with
//!   `KILL_ON_JOB_CLOSE`, so closing/crashing the client tears the sidecar
//!   down with it.
//! - On every spawn we also reap any stale `metroforge-sidecar` left behind
//!   by a previous unclean exit.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
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

/// Bytes retained from the sidecar's stderr for the fatal-error diagnostics
/// screen. Enough to cover a typical crash dump without ballooning memory.
const LOG_TAIL_BYTES: usize = 16 * 1024;

/// Why the sim link was declared dead. Distinguishes an OS-level process
/// exit (immediate) from a hung-but-still-running process that stopped
/// talking on the WebSocket (silence window in `ws_transport`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidecarDeathReason {
    /// `Child::try_wait` reported the process has exited.
    ProcessExited { code: Option<i32> },
    /// Transport saw no inbound frames for longer than the liveness window.
    WebsocketSilence { silence_ms: u64 },
}

impl SidecarDeathReason {
    pub fn label(&self) -> &'static str {
        match self {
            SidecarDeathReason::ProcessExited { .. } => "process exited",
            SidecarDeathReason::WebsocketSilence { .. } => "websocket silence",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            SidecarDeathReason::ProcessExited { code } => match code {
                Some(c) => format!("sidecar process exited with code {c}"),
                None => "sidecar process exited (no exit code)".to_string(),
            },
            SidecarDeathReason::WebsocketSilence { silence_ms } => {
                format!("no websocket traffic for {silence_ms} ms")
            }
        }
    }
}

/// Shared ring buffer filled by the stderr reader thread.
#[derive(Clone, Default)]
pub struct SidecarLogTail {
    inner: Arc<Mutex<VecDeque<u8>>>,
}

impl SidecarLogTail {
    fn append(&self, chunk: &[u8]) {
        let Ok(mut buf) = self.inner.lock() else {
            return;
        };
        for &b in chunk {
            if buf.len() >= LOG_TAIL_BYTES {
                buf.pop_front();
            }
            buf.push_back(b);
        }
    }

    /// Lossy UTF-8 view of the retained stderr bytes (newest `LOG_TAIL_BYTES`).
    pub fn as_string(&self) -> String {
        let Ok(buf) = self.inner.lock() else {
            return String::new();
        };
        let bytes: Vec<u8> = buf.iter().copied().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

pub struct SidecarProcess {
    child: Child,
    pub port: u16,
    log_tail: SidecarLogTail,
    /// Windows Job Object handle; kept alive so `KILL_ON_JOB_CLOSE` fires
    /// when this struct (and thus the client) goes away.
    #[cfg(target_os = "windows")]
    _job: Option<windows_job::JobHandle>,
}

impl SidecarProcess {
    /// Lookup order (spec §3.2 `sidecar.rs`):
    /// 1. `$MF_SIDECAR_PATH` (exact binary path)
    /// 2. a `metroforge-sidecar[.exe]` next to the running exe
    /// 3. dev fallback: `bun run sidecar/index.ts` with cwd `/root/metroforge`
    ///
    /// `headless_speed`, if set, is passed as `--headless-speed <n>`.
    pub fn spawn(headless_speed: Option<f64>) -> anyhow::Result<Self> {
        // Reap anything left behind by a previous client crash before we
        // bind a fresh port — otherwise a zombie can hold resources or
        // confuse diagnostics.
        kill_stale_sidecars();

        let (mut cmd, launch_desc) = Self::resolve_launch()?;
        cmd.arg("--port").arg("0");
        if let Some(speed) = headless_speed {
            cmd.arg("--headless-speed").arg(speed.to_string());
        }
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());

        configure_orphan_prevention(&mut cmd)?;

        let mut child = cmd
            .spawn()
            .map_err(|e| anyhow::anyhow!("failed to spawn sidecar ({launch_desc}): {e}"))?;

        #[cfg(target_os = "windows")]
        let job = match windows_job::assign_child_to_kill_on_close_job(&mut child) {
            Ok(job) => Some(job),
            Err(e) => {
                tracing::warn!("mf-net: failed to assign sidecar to job object: {e}");
                None
            }
        };

        let log_tail = SidecarLogTail::default();
        if let Some(stderr) = child.stderr.take() {
            spawn_stderr_reader(stderr, log_tail.clone());
        }

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
            log_tail,
            #[cfg(target_os = "windows")]
            _job: job,
        })
    }

    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}", self.port)
    }

    /// Non-blocking poll: `Some` once the OS reports the child has exited.
    pub fn try_exit_status(&mut self) -> Option<std::process::ExitStatus> {
        match self.child.try_wait() {
            Ok(status) => status,
            Err(e) => {
                tracing::warn!("mf-net: try_wait on sidecar failed: {e}");
                None
            }
        }
    }

    pub fn log_tail(&self) -> String {
        self.log_tail.as_string()
    }

    /// Best-effort kill used by the reconnect path before respawning, and by
    /// the `MF_TEST_KILL_SIDECAR` harness. Prefer this over dropping the
    /// whole `SimLink` when the transport must die first.
    pub fn kill_now(&mut self) {
        kill_child_tree(&mut self.child);
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
        kill_child_tree(&mut self.child);
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

fn spawn_stderr_reader(stderr: std::process::ChildStderr, log_tail: SidecarLogTail) {
    std::thread::Builder::new()
        .name("mf-net-sidecar-stderr".into())
        .spawn(move || {
            // Tee: in-memory tail (fatal diagnostics screen, this PR) plus
            // the rotating on-disk log (#80) from one reader thread.
            let mut disk = sidecar_stderr_log_path().and_then(|p| {
                if let Some(parent) = p.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                RotatingLog::open(&p, STDERR_LOG_MAX_BYTES).ok()
            });
            let mut reader = BufReader::new(stderr);
            let mut buf = [0u8; 1024];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        log_tail.append(&buf[..n]);
                        if let Some(d) = disk.as_mut() {
                            let _ = d.write_chunk(&buf[..n]);
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .expect("failed to spawn sidecar stderr reader");
}

/// Rotate `sidecar-stderr.log` once it exceeds this many bytes (#80).
const STDERR_LOG_MAX_BYTES: u64 = 512 * 1024;

fn sidecar_logs_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "[REDACTED]", "MetroForge")
        .map(|dirs| dirs.data_dir().join("logs"))
}

fn sidecar_stderr_log_path() -> Option<PathBuf> {
    sidecar_logs_dir().map(|d| d.join("sidecar-stderr.log"))
}

/// Append-only log that renames to `*.1` and starts fresh past `max_bytes`
/// (#80's rotating stderr sink, fed here by the tee above).
struct RotatingLog {
    path: PathBuf,
    max_bytes: u64,
    file: File,
    written: u64,
}

impl RotatingLog {
    fn open(path: &std::path::Path, max_bytes: u64) -> std::io::Result<Self> {
        let written = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            path: path.to_path_buf(),
            max_bytes,
            file,
            written,
        })
    }

    fn write_chunk(&mut self, chunk: &[u8]) -> std::io::Result<()> {
        if self.written >= self.max_bytes {
            self.rotate()?;
        }
        self.file.write_all(chunk)?;
        self.written = self.written.saturating_add(chunk.len() as u64);
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        self.file.flush()?;
        let rotated = self.path.with_extension("log.1");
        let _ = std::fs::remove_file(&rotated);
        let _ = std::fs::rename(&self.path, &rotated);
        self.file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        self.written = 0;
        Ok(())
    }
}

/// Platform hooks that keep a crashed client from leaving a zombie sidecar.
fn configure_orphan_prevention(cmd: &mut Command) -> anyhow::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group so Drop can signal the whole tree (bun → node,
        // etc.), and PR_SET_PDEATHSIG so a hard client kill still reaps us.
        unsafe {
            cmd.pre_exec(|| {
                // New process group with this pid as leader.
                if libc::setpgid(0, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Survive execve; when the parent dies the kernel delivers
                // SIGTERM to this process.
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                // Close the race where the parent died between fork and
                // prctl: if ppid is already 1, suicide now.
                if libc::getppid() == 1 {
                    libc::raise(libc::SIGTERM);
                }
                Ok(())
            });
        }
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: Bun-compiled sidecar is a console-subsystem exe;
        // without this, launching the game pops a second empty console.
        // CREATE_BREAKAWAY_FROM_JOB is intentionally NOT set — we want the
        // child assignable to our kill-on-close job below.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let _ = cmd;
    Ok(())
}

fn kill_child_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // Negative pgid = signal the whole group we created in pre_exec.
        unsafe {
            let _ = libc::kill(-pid, libc::SIGTERM);
        }
        // Brief grace, then escalate — Drop must not block the client for
        // long on a wedged child.
        std::thread::sleep(Duration::from_millis(50));
        unsafe {
            let _ = libc::kill(-pid, libc::SIGKILL);
        }
        let _ = child.wait();
    }

    #[cfg(not(unix))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Kill any leftover `metroforge-sidecar` processes from a previous run.
/// Best-effort and silent on failure — a permissions error must not block
/// a fresh spawn.
pub fn kill_stale_sidecars() {
    #[cfg(unix)]
    {
        // `pkill -x` matches the exact process name; ignore status (no
        // matches → non-zero, which is fine).
        let _ = Command::new("pkill")
            .args(["-x", "metroforge-sidecar"])
            .status();
        // Dev-fallback bun processes are harder to fingerprint safely; the
        // PDEATHSIG / process-group path covers the common orphan case.
    }

    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "metroforge-sidecar.exe"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Windows Job Object helpers — kept in a submodule so the rest of this
/// file stays readable without `cfg` noise on every line.
#[cfg(target_os = "windows")]
mod windows_job {
    use std::process::Child;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::OpenProcess;
    use windows_sys::Win32::System::Threading::PROCESS_ALL_ACCESS;

    pub struct JobHandle(HANDLE);

    // SAFETY: a Job Object HANDLE is a kernel object reference with no
    // thread affinity; Win32 allows using it from any thread. We only
    // close it (Drop) and never alias interior state. Required because
    // SimLink (a Bevy Resource) holds the handle so the job lives as long
    // as the sim link.
    unsafe impl Send for JobHandle {}
    unsafe impl Sync for JobHandle {}

    impl Drop for JobHandle {
        fn drop(&mut self) {
            // Closing the last handle to a kill-on-close job terminates
            // every process still assigned to it — that's the orphan fix.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    pub fn assign_child_to_kill_on_close_job(child: &mut Child) -> anyhow::Result<JobHandle> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                anyhow::bail!("CreateJobObjectW failed");
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut _,
                std::mem::size_of_val(&info) as u32,
            );
            if ok == 0 {
                let _ = CloseHandle(job);
                anyhow::bail!("SetInformationJobObject failed");
            }

            // `Child` doesn't expose a raw HANDLE on all Rust versions
            // without the windows `ChildExt`; open by pid instead.
            let process = OpenProcess(PROCESS_ALL_ACCESS, 0, child.id());
            if process.is_null() {
                let _ = CloseHandle(job);
                anyhow::bail!("OpenProcess failed for sidecar pid {}", child.id());
            }
            let assigned = AssignProcessToJobObject(job, process);
            let _ = CloseHandle(process);
            if assigned == 0 {
                let _ = CloseHandle(job);
                anyhow::bail!("AssignProcessToJobObject failed");
            }
            Ok(JobHandle(job))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn death_reason_labels_distinguish_exit_from_silence() {
        assert_eq!(
            SidecarDeathReason::ProcessExited { code: Some(1) }.label(),
            "process exited"
        );
        assert_eq!(
            SidecarDeathReason::WebsocketSilence { silence_ms: 5000 }.label(),
            "websocket silence"
        );
    }

    #[test]
    fn log_tail_ring_retains_newest_bytes_only() {
        let tail = SidecarLogTail::default();
        let big = vec![b'a'; LOG_TAIL_BYTES + 100];
        tail.append(&big);
        let s = tail.as_string();
        assert_eq!(s.len(), LOG_TAIL_BYTES);
        assert!(s.chars().all(|c| c == 'a'));
    }
}
