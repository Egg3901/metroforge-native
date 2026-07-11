//! Panic hook that writes a crash report under the OS local data dir
//! (`paths::crash_reports_dir()`, i.e. Local AppData on Windows) so a
//! hard crash leaves something inspectable next to saves/config rather
//! than only a vanished console (release builds use the Windows GUI
//! subsystem and have no console).
use std::io::Write;
use std::panic;

use crate::paths;

/// Install a panic hook that appends a timestamped report file, then
/// chains to the previous hook (usually the default printer).
pub fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if let Err(e) = write_crash_report(info) {
            // Best-effort: never let the hook itself panic.
            let _ = writeln!(
                std::io::stderr(),
                "mf-game: failed to write crash report: {e}"
            );
        }
        previous(info);
    }));
}

fn write_crash_report(info: &panic::PanicHookInfo<'_>) -> std::io::Result<()> {
    let Some(dir) = paths::crash_reports_dir() else {
        return Ok(());
    };
    std::fs::create_dir_all(&dir)?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{ts}.txt"));

    let mut file = std::fs::File::create(&path)?;
    writeln!(file, "MetroForge crash report")?;
    writeln!(file, "version: {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(file, "target: {}", std::env::consts::OS)?;
    writeln!(file, "timestamp_unix_secs: {ts}")?;
    writeln!(file)?;
    writeln!(file, "{info}")?;
    if let Some(location) = info.location() {
        writeln!(
            file,
            "location: {}:{}:{}",
            location.file(),
            location.line(),
            location.column()
        )?;
    }
    tracing::error!("mf-game: panic; crash report written to {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crash_reports_dir_is_under_local_data() {
        let dir = paths::crash_reports_dir().expect("crash_reports_dir");
        assert!(dir.ends_with("crash-reports"));
    }
}
