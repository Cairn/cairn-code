use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn exec_mode_emits_stream_json_events() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_cairn-code"))
        .args([
            "exec",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--init-session-id",
            "test-exec-session-123",
        ])
        .env("CAIRN_PROVIDER", "ollama")
        .env("CAIRN_MODEL", "cairn-exec-test-model")
        .env("OLLAMA_HOST", "http://127.0.0.1:1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn cairn-code exec");

    let input_msg = serde_json::json!({
        "schemaVersion": 2,
        "type": "message",
        "role": "user",
        "content": "hello cairn exec"
    })
    .to_string();

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(format!("{input_msg}\n").as_bytes())
        .expect("write json input");

    let output = child.wait_with_output().expect("wait for cairn-code exec");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("\"type\":\"run_start\""),
        "stdout missing run_start: {stdout}"
    );
    assert!(
        stdout.contains("test-exec-session-123"),
        "stdout missing session ID: {stdout}"
    );
    assert!(
        stdout.contains("\"type\":\"run_end\""),
        "stdout missing run_end: {stdout}"
    );
}
