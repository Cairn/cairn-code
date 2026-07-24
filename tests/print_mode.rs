use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn unknown_option_uses_stderr_and_nonzero_status() {
    let output = Command::new(env!("CARGO_BIN_EXE_cairn-code"))
        .arg("--unknown")
        .output()
        .expect("run cairn-code");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "unexpected success: {output:?}");
    assert!(output.stdout.is_empty(), "unexpected stdout: {output:?}");
    assert!(stderr.contains("unknown option '--unknown'"), "{stderr}");
    assert!(stderr.contains("--help"), "{stderr}");
}

#[test]
fn multiple_prompts_use_stderr_and_nonzero_status() {
    let output = Command::new(env!("CARGO_BIN_EXE_cairn-code"))
        .args(["first prompt", "second prompt"])
        .output()
        .expect("run cairn-code");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "unexpected success: {output:?}");
    assert!(output.stdout.is_empty(), "unexpected stdout: {output:?}");
    assert!(
        stderr.contains("unexpected positional argument 'second prompt'"),
        "{stderr}"
    );
    assert!(stderr.contains("one quoted argument"), "{stderr}");
}

#[test]
fn stdin_provider_failure_uses_stderr_and_nonzero_status() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cairn-code"))
        .arg("--print")
        .env("CAIRN_PROVIDER", "ollama")
        .env("CAIRN_MODEL", "cairn-print-mode-test-model")
        .env("OLLAMA_HOST", "http://127.0.0.1:1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cairn-code");

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(b"prompt supplied on stdin\n")
        .expect("write prompt");

    let output = child.wait_with_output().expect("wait for cairn-code");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "unexpected success: {output:?}");
    assert!(output.stdout.is_empty(), "unexpected stdout: {output:?}");
    assert!(stderr.starts_with("Error: LLM error:"), "{stderr}");
}
