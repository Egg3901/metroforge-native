//! Local crash reporting: panic hook, log ring buffer, next-launch notice,
//! and `--safe-mode` (Potato + weather/bloom/outlines off). Reports stay on
//! disk only — no network transmission, no account/PII fields.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use bevy::log::{BoxedLayer, LogPlugin};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPrimaryContextPass};
use mf_state::{GpuDeviceKind, QualityTier, WeatherEffects};

use crate::design_system;
use crate::state::AppState;

/// How many recent log lines the panic hook embeds in a crash report.
pub const LOG_RING_CAPACITY: usize = 200;

/// Marker filename: present means the next launch should show the notice.
const NOTICE_MARKER: &str = "SHOW_NOTICE";
/// Canonical last-session report name (also kept under a timestamped copy).
const LAST_CRASH_FILE: &str = "last_session.txt";

static LOG_RING: OnceLock<Mutex<LogRing>> = OnceLock::new();
static GPU_ADAPTER: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn log_ring() -> &'static Mutex<LogRing> {
    LOG_RING.get_or_init(|| Mutex::new(LogRing::new(LOG_RING_CAPACITY)))
}

fn gpu_adapter_slot() -> &'static Mutex<Option<String>> {
    GPU_ADAPTER.get_or_init(|| Mutex::new(None))
}

/// Fixed-capacity ring of recent log lines for crash reports.
#[derive(Debug, Clone)]
pub struct LogRing {
    lines: VecDeque<String>,
    capacity: usize,
}

impl LogRing {
    pub fn new(capacity: usize) -> Self {
        Self {
            lines: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, line: impl Into<String>) {
        if self.capacity == 0 {
            return;
        }
        if self.lines.len() == self.capacity {
            self.lines.pop_front();
        }
        self.lines.push_back(line.into());
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.lines.iter().cloned().collect()
    }
}

/// Privacy-clean crash report payload (local disk only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashReport {
    pub panic_message: String,
    pub backtrace: String,
    pub os_info: String,
    pub gpu_adapter: Option<String>,
    pub game_version: String,
    pub log_lines: Vec<String>,
}

impl CrashReport {
    /// Human-readable report body. No network URLs, emails, or user paths
    /// beyond the OS/arch line and adapter name already known to the GPU.
    pub fn serialize(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "MetroForge crash report");
        let _ = writeln!(out, "version: {}", self.game_version);
        let _ = writeln!(out, "os: {}", self.os_info);
        match &self.gpu_adapter {
            Some(gpu) => {
                let _ = writeln!(out, "gpu: {gpu}");
            }
            None => {
                let _ = writeln!(out, "gpu: (not yet detected)");
            }
        }
        let _ = writeln!(out);
        let _ = writeln!(out, "=== panic ===");
        let _ = writeln!(out, "{}", self.panic_message);
        let _ = writeln!(out);
        let _ = writeln!(out, "=== backtrace ===");
        let _ = writeln!(out, "{}", self.backtrace);
        let _ = writeln!(out);
        let _ = writeln!(out, "=== last {} log lines ===", self.log_lines.len());
        for line in &self.log_lines {
            let _ = writeln!(out, "{line}");
        }
        out
    }

    pub fn from_panic(panic_message: String) -> Self {
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let gpu_adapter = gpu_adapter_slot().lock().ok().and_then(|g| g.clone());
        let log_lines = log_ring().lock().map(|r| r.snapshot()).unwrap_or_default();
        Self {
            panic_message,
            backtrace,
            os_info: os_info_line(),
            gpu_adapter,
            game_version: env!("CARGO_PKG_VERSION").to_string(),
            log_lines,
        }
    }
}

fn os_info_line() -> String {
    format!(
        "{} {} ({})",
        std::env::consts::OS,
        std::env::consts::ARCH,
        std::env::consts::FAMILY
    )
}

/// Project data dir helper (same qualifier as config/saves).
pub fn project_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("com", "[REDACTED]", "MetroForge")
        .map(|d| d.data_dir().to_path_buf())
}

pub fn crashes_dir() -> Option<PathBuf> {
    project_data_dir().map(|d| d.join("crashes"))
}

fn notice_marker_path(dir: &Path) -> PathBuf {
    dir.join(NOTICE_MARKER)
}

fn last_crash_path(dir: &Path) -> PathBuf {
    dir.join(LAST_CRASH_FILE)
}

/// Write a crash report under the OS data dir and arm the next-launch notice.
pub fn write_crash_report(report: &CrashReport) -> Option<PathBuf> {
    let dir = crashes_dir()?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("mf-game: could not create crashes dir: {e}");
        return None;
    }
    let body = report.serialize();
    let last = last_crash_path(&dir);
    if let Err(e) = std::fs::write(&last, &body) {
        eprintln!("mf-game: could not write crash report: {e}");
        return None;
    }
    // Timestamped copy so older crashes remain diagnosable.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stamped = dir.join(format!("crash-{stamp}.txt"));
    let _ = std::fs::write(&stamped, &body);
    let _ = std::fs::write(notice_marker_path(&dir), b"");
    Some(last)
}

/// Record GPU adapter details once quality-boot (or safe-mode) knows them.
pub fn record_gpu_adapter(name: &str, kind: GpuDeviceKind) {
    if let Ok(mut slot) = gpu_adapter_slot().lock() {
        *slot = Some(format!("{name} ({kind:?})"));
    }
}

fn push_log_line(line: String) {
    if let Ok(mut ring) = log_ring().lock() {
        ring.push(line);
    }
}

/// Install the process panic hook. Safe to call once before `App::run`.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = panic_message(info);
        let report = CrashReport::from_panic(message);
        let _ = write_crash_report(&report);
        previous(info);
    }));
}

fn panic_message(info: &std::panic::PanicHookInfo<'_>) -> String {
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "(unknown location)".to_string());
    let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "Box<dyn Any>".to_string()
    };
    format!("{payload}\n  at {location}")
}

/// `LogPlugin::custom_layer` entry: retain the last [`LOG_RING_CAPACITY`] lines.
pub fn log_layer(_app: &mut App) -> Option<BoxedLayer> {
    Some(Box::new(RingBufferLayer))
}

struct RingBufferLayer;

impl<S> bevy::log::tracing_subscriber::Layer<S> for RingBufferLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: bevy::log::tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut message = String::new();
        event.record(&mut MessageVisitor(&mut message));
        let meta = event.metadata();
        let line = if message.is_empty() {
            format!("[{}] {}", meta.level(), meta.target())
        } else {
            format!("[{}] {}: {message}", meta.level(), meta.target())
        };
        push_log_line(line);
    }
}

struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0.push_str(value);
        } else if !self.0.is_empty() {
            let _ = write!(self.0, " {}={value}", field.name());
        } else {
            let _ = write!(self.0, "{}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.0, "{value:?}");
        } else if !self.0.is_empty() {
            let _ = write!(self.0, " {}={value:?}", field.name());
        } else {
            let _ = write!(self.0, "{}={value:?}", field.name());
        }
    }
}

/// CLI / session flag: force Potato and disable weather (bloom/outlines follow
/// Potato knobs).
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SafeMode(pub bool);

/// Parsed once at process start from `--safe-mode`.
#[derive(Debug, Clone, Copy, Default)]
pub struct CrashCli {
    pub safe_mode: bool,
}

pub fn parse_cli(args: impl IntoIterator<Item = String>) -> CrashCli {
    let mut cli = CrashCli::default();
    for arg in args {
        if arg == "--safe-mode" {
            cli.safe_mode = true;
        }
    }
    cli
}

/// Pending next-launch crash notice (dismissible overlay).
#[derive(Resource, Debug, Clone)]
pub struct CrashNotice {
    pub crashes_dir: PathBuf,
    pub report_path: PathBuf,
    pub visible: bool,
}

/// True when the process is running under an automated harness (CI smoke,
/// soak, screenshot/promo capture, gallery) rather than an interactive player.
/// These runs share the same OS data dir, so a marker left by an earlier real
/// crash must not force them to boot into the crash dialog instead of the game.
const HARNESS_VARS: &[&str] = &[
    "CI",
    "MF_AUTOSTART",
    "MF_SOAK",
    "MF_VERIFY_DIR",
    "MF_PROMO_DIR",
    "MF_ATMOSPHERE_DIR",
    "MF_UI_GALLERY",
    "MF_MENU_SCREEN",
];

fn harness_run() -> bool {
    harness_run_with(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

/// Pure core of [`harness_run`]: true when any harness var reads as set.
fn harness_run_with(present: impl Fn(&str) -> bool) -> bool {
    HARNESS_VARS.iter().any(|k| present(k))
}

impl CrashNotice {
    /// Load from disk if the previous session left a notice marker.
    pub fn detect() -> Option<Self> {
        let dir = crashes_dir()?;
        let marker = notice_marker_path(&dir);
        if !marker.is_file() {
            return None;
        }
        // A shared data dir means an earlier real crash can leave a marker that
        // then poisons every later automated run. Under a harness, consume the
        // stale marker (so it stops re-triggering) and never show the dialog.
        if harness_run() {
            let _ = std::fs::remove_file(&marker);
            return None;
        }
        let report_path = last_crash_path(&dir);
        Some(Self {
            crashes_dir: dir,
            report_path,
            visible: true,
        })
    }

    pub fn dismiss(&mut self) {
        self.visible = false;
        let marker = notice_marker_path(&self.crashes_dir);
        let _ = std::fs::remove_file(marker);
    }
}

pub struct MfCrashPlugin;

impl Plugin for MfCrashPlugin {
    fn build(&self, app: &mut App) {
        if let Some(notice) = CrashNotice::detect() {
            app.insert_resource(notice);
        }
        app.add_systems(
            EguiPrimaryContextPass,
            crash_notice_ui_system
                .run_if(resource_exists::<CrashNotice>)
                .run_if(|| !design_system::hud_hidden()),
        );
    }
}

/// Default [`LogPlugin`] with the crash ring-buffer layer attached.
pub fn log_plugin_with_ring() -> LogPlugin {
    LogPlugin {
        custom_layer: log_layer,
        ..default()
    }
}

fn crash_notice_ui_system(
    mut contexts: EguiContexts,
    mut notice: ResMut<CrashNotice>,
    mut safe_mode: ResMut<SafeMode>,
    mut quality: ResMut<QualityTier>,
    mut weather: ResMut<WeatherEffects>,
    state: Res<State<AppState>>,
) -> Result {
    if !notice.visible {
        return Ok(());
    }
    // Never block boot/connect; show once the shell is interactive.
    if matches!(
        *state.get(),
        AppState::Boot | AppState::ConnectingSim | AppState::Loading
    ) {
        return Ok(());
    }

    let ctx = contexts.ctx_mut()?;
    let mut open_location = false;
    let mut continue_clicked = false;
    let mut safe_clicked = false;
    let report_path_display = notice
        .report_path
        .is_file()
        .then(|| notice.report_path.display().to_string());
    let mut window_open = notice.visible;

    egui::Window::new("Crash report")
        .id(egui::Id::new("mf_crash_notice"))
        .open(&mut window_open)
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-16.0, 16.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            ui.set_width(280.0);
            ui.label(
                egui::RichText::new("MetroForge crashed last session")
                    .size(15.0)
                    .strong(),
            );
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "A local report was saved on this device. Nothing was sent over the network.",
                )
                .size(12.0)
                .color(design_system::current_colors().muted),
            );
            if let Some(path) = &report_path_display {
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(path)
                        .size(10.0)
                        .color(design_system::current_colors().muted),
                );
            }
            ui.add_space(12.0);
            if ui
                .add_sized([260.0, 32.0], egui::Button::new("Open report location"))
                .clicked()
            {
                open_location = true;
            }
            ui.add_space(6.0);
            if ui
                .add_sized([260.0, 32.0], egui::Button::new("Safe mode"))
                .clicked()
            {
                safe_clicked = true;
            }
            ui.add_space(6.0);
            if ui
                .add_sized(
                    [260.0, 36.0],
                    egui::Button::new(
                        egui::RichText::new("Continue")
                            .color(egui::Color32::WHITE)
                            .strong(),
                    )
                    .fill(design_system::current_colors().accent),
                )
                .clicked()
            {
                continue_clicked = true;
            }
        });

    if open_location {
        open_in_file_manager(&notice.crashes_dir);
    }
    if safe_clicked {
        apply_safe_mode(&mut safe_mode, &mut quality, &mut weather);
        notice.dismiss();
    }
    if continue_clicked || !window_open {
        notice.dismiss();
    }
    Ok(())
}

/// Force Potato and disable weather for this session (bloom/outlines follow
/// Potato knobs). Session only: does not rewrite `config.toml`.
pub fn apply_safe_mode(
    safe_mode: &mut SafeMode,
    quality: &mut QualityTier,
    weather: &mut WeatherEffects,
) {
    safe_mode.0 = true;
    *quality = QualityTier::Potato;
    weather.enabled = false;
    tracing::info!("mf-game: safe mode on (Potato, weather off)");
}

fn open_in_file_manager(path: &Path) {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let result = {
            #[cfg(target_os = "windows")]
            {
                std::process::Command::new("explorer").arg(&path).spawn()
            }
            #[cfg(target_os = "macos")]
            {
                std::process::Command::new("open").arg(&path).spawn()
            }
            #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
            {
                std::process::Command::new("xdg-open").arg(&path).spawn()
            }
        };
        if let Err(e) = result {
            tracing::warn!("mf-game: could not open crash report folder: {e}");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_buffer_drops_oldest_when_full() {
        let mut ring = LogRing::new(3);
        ring.push("a");
        ring.push("b");
        ring.push("c");
        ring.push("d");
        assert_eq!(ring.snapshot(), vec!["b", "c", "d"]);
    }

    #[test]
    fn ring_buffer_zero_capacity_stays_empty() {
        let mut ring = LogRing::new(0);
        ring.push("x");
        assert!(ring.snapshot().is_empty());
    }

    #[test]
    fn ring_buffer_snapshot_preserves_order() {
        let mut ring = LogRing::new(10);
        ring.push("one");
        ring.push("two");
        assert_eq!(ring.snapshot(), vec!["one".to_string(), "two".to_string()]);
    }

    #[test]
    fn crash_report_serialize_contains_required_sections() {
        let report = CrashReport {
            panic_message: "boom\n  at file.rs:1:1".to_string(),
            backtrace: "stack frame here".to_string(),
            os_info: "linux x86_64 (unix)".to_string(),
            gpu_adapter: Some("Mesa Lavapipe (Cpu)".to_string()),
            game_version: "0.4.4".to_string(),
            log_lines: vec!["[INFO] hello".to_string(), "[WARN] careful".to_string()],
        };
        let text = report.serialize();
        assert!(text.contains("MetroForge crash report"));
        assert!(text.contains("version: 0.4.4"));
        assert!(text.contains("os: linux x86_64 (unix)"));
        assert!(text.contains("gpu: Mesa Lavapipe (Cpu)"));
        assert!(text.contains("=== panic ==="));
        assert!(text.contains("boom"));
        assert!(text.contains("=== backtrace ==="));
        assert!(text.contains("stack frame here"));
        assert!(text.contains("=== last 2 log lines ==="));
        assert!(text.contains("[INFO] hello"));
        assert!(text.contains("[WARN] careful"));
        // Privacy: no outbound URL / telemetry wording in the body template.
        assert!(!text.to_lowercase().contains("http"));
        assert!(!text.to_lowercase().contains("upload"));
        assert!(!text.to_lowercase().contains("telemetry"));
    }

    #[test]
    fn crash_report_serialize_handles_missing_gpu() {
        let report = CrashReport {
            panic_message: "early".to_string(),
            backtrace: "bt".to_string(),
            os_info: "os".to_string(),
            gpu_adapter: None,
            game_version: "1.0.0".to_string(),
            log_lines: vec![],
        };
        let text = report.serialize();
        assert!(text.contains("gpu: (not yet detected)"));
        assert!(text.contains("=== last 0 log lines ==="));
    }

    #[test]
    fn harness_run_detects_any_known_var() {
        assert!(!harness_run_with(|_| false));
        for var in HARNESS_VARS {
            let target = *var;
            assert!(
                harness_run_with(|k| k == target),
                "{target} should mark a harness run"
            );
        }
        // An unrelated var must not trip the harness guard.
        assert!(!harness_run_with(|k| k == "PATH"));
    }

    #[test]
    fn parse_cli_detects_safe_mode() {
        assert!(!parse_cli(["metroforge".into()]).safe_mode);
        assert!(parse_cli(["metroforge".into(), "--safe-mode".into()]).safe_mode);
        assert!(parse_cli(["--safe-mode".into(), "other".into()]).safe_mode);
    }

    #[test]
    fn player_facing_crash_copy_is_dash_free() {
        let copy = [
            "MetroForge crashed last session",
            "A local report was saved on this device. Nothing was sent over the network.",
            "Open report location",
            "Safe mode",
            "Continue",
        ];
        for text in copy {
            assert!(!text.contains('-'), "{text:?} contains a dash");
            assert!(!text.contains('\u{2013}'), "{text:?} contains an en dash");
            assert!(!text.contains('\u{2014}'), "{text:?} contains an em dash");
        }
    }
}
