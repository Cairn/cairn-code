//! Lightweight attention sounds when the agent finishes or needs input.
//!
//! Primary cue is the terminal BEL (`\x07`), which Windows Terminal maps to
//! its notification chime. Extra platform tones are only used where they do
//! not stack a second ugly PC-speaker beep on top of that chime.
//! Disable with `CAIRN_SOUND=0` (or `false` / `off` / `no`).

use std::io::{self, Write};
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::thread;
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
/// of spawn overhead; multi-tone patterns run on a detached thread.
pub fn play(kind: Kind) {
    if !enabled() {
        return;
    }
    // Terminal BEL: Windows Terminal plays its soft flourish; other terminals
    // typically map this to their own bell/alert. Do not layer Console.Beep
    // on Windows — that is the square-wave POST-speaker tone.
    let _ = write!(io::stderr(), "\x07");
    let _ = io::stderr().flush();

    thread::spawn(move || match kind {
        Kind::Done => platform_extra_done(),
        Kind::Attention => platform_extra_attention(),
    });
}

fn bell() {
    let _ = write!(io::stderr(), "\x07");
    let _ = io::stderr().flush();
}

#[cfg(windows)]
fn platform_extra_done() {
    // BEL alone is enough: Windows Terminal already sounds the nice chime.
}

#[cfg(windows)]
fn platform_extra_attention() {
    // Distinct from Done: a second soft BEL, still no Console.Beep.
    thread::sleep(Duration::from_millis(120));
    bell();
}

#[cfg(target_os = "macos")]
fn platform_extra_done() {
    // Soft system sound; BEL already fired above for terminals that use it.
    let _ = Command::new("afplay")
        .args(["-v", "0.3", "/System/Library/Sounds/Pop.aiff"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(target_os = "macos")]
fn platform_extra_attention() {
    let _ = Command::new("afplay")
        .args(["-v", "0.35", "/System/Library/Sounds/Funk.aiff"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_extra_done() {
    // Second BEL after a short gap if the terminal supports it.
    thread::sleep(Duration::from_millis(40));
    bell();
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_extra_attention() {
    for _ in 0..2 {
        thread::sleep(Duration::from_millis(90));
        bell();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_defaults_true_without_env() {
        let _guard = crate::test_support::ENV_LOCK.lock().unwrap();
        let _sound_env = crate::test_support::EnvGuard::capture("CAIRN_SOUND");
        // Do not assert absolute truth under env pollution; just ensure the
        // parser treats explicit off values as disabled.
        std::env::set_var("CAIRN_SOUND", "0");
        assert!(!enabled());
        std::env::set_var("CAIRN_SOUND", "false");
        assert!(!enabled());
        std::env::set_var("CAIRN_SOUND", "1");
        assert!(enabled());
    }

    #[test]
    fn play_does_not_panic() {
        let _guard = crate::test_support::ENV_LOCK.lock().unwrap();
        let _sound_env = crate::test_support::EnvGuard::capture("CAIRN_SOUND");
        std::env::set_var("CAIRN_SOUND", "0");
        play(Kind::Done);
        play(Kind::Attention);
    }
}
