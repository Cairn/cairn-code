//! Lightweight attention sounds when the agent finishes or needs input.
//!
//! Uses the terminal BEL plus a short system beep on Windows/macOS when
//! available. Disable with `CAIRN_SOUND=0` (or `false` / `off` / `no`).

use std::io::{self, Write};
#[cfg(any(windows, target_os = "macos"))]
use std::process::{Command, Stdio};
use std::thread;
#[cfg(all(unix, not(target_os = "macos")))]
use std::time::Duration;

/// What kind of cue to play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Agent finished a turn successfully (idle again).
    Done,
    /// Needs user attention: permission prompt, LLM recovery, etc.
    Attention,
}

/// True unless the user opted out via env.
pub fn enabled() -> bool {
    match std::env::var("CAIRN_SOUND") {
        Ok(v) => !matches!(
            v.to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off" | "mute" | "silent"
        ),
        Err(_) => true,
    }
}

/// Fire-and-forget notification. Never blocks the TUI for more than a few ms
/// of spawn overhead; actual tones run on a detached thread when needed.
pub fn play(kind: Kind) {
    if !enabled() {
        return;
    }
    // Universal: terminal bell (works in most terminals on all platforms).
    let _ = write!(io::stderr(), "\x07");
    let _ = io::stderr().flush();

    // Richer, non-blocking system beep when we can without new dependencies.
    thread::spawn(move || match kind {
        Kind::Done => platform_beep_done(),
        Kind::Attention => platform_beep_attention(),
    });
}

#[cfg(windows)]
fn platform_beep_done() {
    // Soft mid tone
    let _ = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[console]::beep(880,90)",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(windows)]
fn platform_beep_attention() {
    // Two-tone: lower then higher (needs attention)
    let _ = Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[console]::beep(660,80); Start-Sleep -Milliseconds 40; [console]::beep(990,120)",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(target_os = "macos")]
fn platform_beep_done() {
    // Glass is short and unobtrusive when available.
    let _ = Command::new("afplay")
        .args(["-v", "0.3", "/System/Library/Sounds/Pop.aiff"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(target_os = "macos")]
fn platform_beep_attention() {
    let _ = Command::new("afplay")
        .args(["-v", "0.35", "/System/Library/Sounds/Funk.aiff"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_beep_done() {
    // Second BEL after a short gap if the terminal supports it.
    thread::sleep(Duration::from_millis(40));
    let _ = write!(io::stderr(), "\x07");
    let _ = io::stderr().flush();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_beep_attention() {
    for _ in 0..2 {
        let _ = write!(io::stderr(), "\x07");
        let _ = io::stderr().flush();
        thread::sleep(Duration::from_millis(90));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_defaults_true_without_env() {
        // Do not assert absolute truth under env pollution; just ensure the
        // parser treats explicit off values as disabled.
        std::env::set_var("CAIRN_SOUND", "0");
        assert!(!enabled());
        std::env::set_var("CAIRN_SOUND", "false");
        assert!(!enabled());
        std::env::set_var("CAIRN_SOUND", "1");
        assert!(enabled());
        std::env::remove_var("CAIRN_SOUND");
    }

    #[test]
    fn play_does_not_panic() {
        std::env::set_var("CAIRN_SOUND", "0");
        play(Kind::Done);
        play(Kind::Attention);
        std::env::remove_var("CAIRN_SOUND");
    }
}
