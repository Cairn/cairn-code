use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use cairn_code::agent::Agent;
use cairn_code::config::Config;
use cairn_code::llm::{self, Usage};
use cairn_code::tools::registry::{Registry, Tool};

struct CliFixture {
    _root: tempfile::TempDir,
    home: PathBuf,
    workspace: PathBuf,
    capture: PathBuf,
}

impl CliFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        Self {
            capture: root.path().join("request.json"),
            _root: root,
            home,
            workspace,
        }
    }

    fn write_config(&self, contents: &str) {
        let directory = self.home.join(".config/cairn-code");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("config.json"), contents).unwrap();
    }

    fn run(&self, prompt: &str, response: &str) -> Output {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let response = response.as_bytes().to_vec();
        let capture = self.capture.clone();
        let server = std::thread::spawn(move || serve_once(listener, &response, &capture));

        let mut command = Command::new(env!("CARGO_BIN_EXE_cairn-code"));
        command
            .args(["--print", prompt])
            .current_dir(&self.workspace)
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("CAIRN_PROVIDER", "ollama")
            .env("CAIRN_MODEL", "llama3.2")
            .env("OLLAMA_HOST", format!("http://{address}"))
            .env("CAIRN_SOUND", "0")
            .env("NO_PROXY", "127.0.0.1,localhost")
            .env("no_proxy", "127.0.0.1,localhost");
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            command.env_remove(name);
        }

        // Supplying every credential prevents startup hydration from consulting
        // the developer's OS keyring while preserving the real CLI code path.
        for name in [
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "OPENROUTER_API_KEY",
            "GITLAWB_OPENGATEWAY_API_KEY",
            "OPENGATEWAY_API_KEY",
            "XAI_API_KEY",
        ] {
            command.env(name, "integration-test-key");
        }

        let output = command.output().unwrap();
        server.join().unwrap();
        output
    }
}

fn serve_once(listener: TcpListener, response: &[u8], capture: &PathBuf) {
    let mut first = true;
    for _ in 0..3 {
        let idle_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if std::time::Instant::now() >= idle_deadline {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(_) => return,
            }
        };
        let mut request = Vec::new();
        let mut buffer = [0; 4096];
        let body_end = loop {
            let read = match stream.read(&mut buffer) {
                Ok(n) if n > 0 => n,
                _ => break 0,
            };
            request.extend_from_slice(&buffer[..read]);
            if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break index + 4;
            }
        };
        if body_end == 0 {
            break;
        }
        let headers = String::from_utf8_lossy(&request[..body_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap_or(0);
        while request.len() - body_end < content_length {
            let read = match stream.read(&mut buffer) {
                Ok(n) if n > 0 => n,
                _ => break,
            };
            request.extend_from_slice(&buffer[..read]);
        }
        if first {
            let _ = fs::write(capture, &request[body_end..body_end + content_length]);
            first = false;
            let resp_header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.len()
            );
            let _ = stream.write_all(resp_header.as_bytes());
            let _ = stream.write_all(response);
        } else {
            let done_resp = r#"{"choices":[{"message":{"role":"assistant","content":"Done."}}]}"#;
            let resp_header = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                done_resp.len()
            );
            let _ = stream.write_all(resp_header.as_bytes());
            let _ = stream.write_all(done_resp.as_bytes());
        }
    }
}

fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn cli_denies_configured_tool_without_writing() {
    let fixture = CliFixture::new();
    fixture.write_config(
        r#"{"permissions":{"auto_allow":["file_read"],"ask":[],"deny":["file_write"]}}"#,
    );
    let response = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-1","function":{"name":"file_write","arguments":"{\"path\":\"blocked.txt\",\"content\":\"owned\"}"}}]}}]}"#;

    let output = stdout(&fixture.run("write blocked.txt", response));

    assert!(
        output.contains("Tool 'file_write' is denied by config"),
        "{output}"
    );
    assert!(!fixture.workspace.join("blocked.txt").exists());
}

#[test]
fn cli_rejects_workspace_traversal_without_disclosing_file() {
    let fixture = CliFixture::new();
    let secret = "outside-secret-must-not-leak";
    fs::write(
        fixture.workspace.parent().unwrap().join("secret.txt"),
        secret,
    )
    .unwrap();
    let response = r#"{"choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call-1","function":{"name":"file_read","arguments":"{\"file_path\":\"../secret.txt\"}"}}]}}]}"#;

    let output = stdout(&fixture.run("read the parent file", response));

    assert!(output.contains("outside the workspace"), "{output}");
    assert!(!output.contains(secret), "{output}");
}

#[test]
fn cli_preserves_unicode_prompt_and_response() {
    let fixture = CliFixture::new();
    let prompt = "Inspect café/東京.rs and the 🏔️ module";
    let response =
        r#"{"choices":[{"message":{"role":"assistant","content":"Grüße from 東京 🏔️"}}]}"#;

    let output = stdout(&fixture.run(prompt, response));

    assert!(output.contains("Grüße from 東京 🏔️"), "{output}");
    let request = fs::read_to_string(&fixture.capture).unwrap();
    assert!(
        request.contains(prompt),
        "request did not preserve prompt: {request}"
    );
}

#[test]
fn corrupt_session_is_rejected_without_losing_neighboring_session() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("bad-session"),
        r#"{"messages":[{"role":"user"}]}"#,
    )
    .unwrap();
    let valid = cairn_code::session::Session {
        id: "good-session".into(),
        model: "test".into(),
        provider: "test".into(),
        messages: vec![],
        tokens_in: 0,
        tokens_out: 0,
        created_at: 0,
        updated_at: 0,
    };
    let directory = root.path().to_string_lossy();
    cairn_code::session::save(&directory, &valid).unwrap();

    assert!(cairn_code::session::load(&directory, "bad-session").is_err());
    assert_eq!(
        cairn_code::session::load(&directory, "good-session")
            .unwrap()
            .id,
        "good-session"
    );
}

struct CountingProvider(Arc<AtomicUsize>);

impl llm::Provider for CountingProvider {
    fn name(&self) -> &str {
        "counting"
    }

    fn default_model(&self) -> &str {
        "counting"
    }

    fn available_models(&self) -> Vec<llm::ModelInfo> {
        vec![]
    }

    fn stream_complete(
        &self,
        _messages: &[llm::Message],
        _tools: &[llm::ToolDefinition],
        _system: &str,
        _model: &str,
        _max_tokens: usize,
        _on_chunk: llm::StreamingCallback,
        _cancel: &AtomicBool,
    ) -> Result<(Vec<llm::Message>, Usage), String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok((vec![], Usage::default()))
    }

    fn complete(
        &self,
        _messages: &[llm::Message],
        _tools: &[llm::ToolDefinition],
        _system: &str,
        _model: &str,
        _max_tokens: usize,
    ) -> Result<(Vec<llm::Message>, Usage), String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok((vec![], Usage::default()))
    }
}

#[test]
fn cancellation_stops_before_provider_or_tool_execution() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut agent = Agent::new(
        Box::new(CountingProvider(calls.clone())),
        "counting".into(),
        Registry::new(),
        Config::default(),
    );
    let (event_tx, _event_rx) = std::sync::mpsc::channel();
    let (_permission_tx, permission_rx) = std::sync::mpsc::channel();
    let cancel = AtomicBool::new(true);

    agent
        .run("must not execute", event_tx, &cancel, &permission_rx)
        .unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

struct CancelingProvider;

impl llm::Provider for CancelingProvider {
    fn name(&self) -> &str {
        "canceling"
    }

    fn default_model(&self) -> &str {
        "canceling"
    }

    fn available_models(&self) -> Vec<llm::ModelInfo> {
        vec![]
    }

    fn stream_complete(
        &self,
        _messages: &[llm::Message],
        _tools: &[llm::ToolDefinition],
        _system: &str,
        _model: &str,
        _max_tokens: usize,
        _on_chunk: llm::StreamingCallback,
        cancel: &AtomicBool,
    ) -> Result<(Vec<llm::Message>, Usage), String> {
        cancel.store(true, Ordering::SeqCst);
        Ok((
            vec![llm::Message {
                role: "assistant".into(),
                content: llm::Content::ToolUse(llm::ToolUse {
                    name: "counting_tool".into(),
                    input: "{}".into(),
                    id: "call-1".into(),
                }),
            }],
            Usage::default(),
        ))
    }

    fn complete(
        &self,
        _messages: &[llm::Message],
        _tools: &[llm::ToolDefinition],
        _system: &str,
        _model: &str,
        _max_tokens: usize,
    ) -> Result<(Vec<llm::Message>, Usage), String> {
        unreachable!()
    }
}

struct CountingTool(Arc<AtomicUsize>);

impl Tool for CountingTool {
    fn name(&self) -> &str {
        "counting_tool"
    }

    fn description(&self) -> &str {
        "Records executions"
    }

    fn input_schema(&self) -> String {
        r#"{"type":"object"}"#.into()
    }

    fn needs_permission(&self) -> bool {
        false
    }

    fn execute(&self, _input: &str) -> Result<String, String> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("executed".into())
    }
}

#[test]
fn cancellation_after_provider_response_stops_tool_execution() {
    let executions = Arc::new(AtomicUsize::new(0));
    let mut registry = Registry::new();
    registry.register(Box::new(CountingTool(executions.clone())));
    let mut agent = Agent::new(
        Box::new(CancelingProvider),
        "canceling".into(),
        registry,
        Config::default(),
    );
    let (event_tx, _event_rx) = std::sync::mpsc::channel();
    let (_permission_tx, permission_rx) = std::sync::mpsc::channel();
    let cancel = AtomicBool::new(false);

    agent
        .run("cancel before tool", event_tx, &cancel, &permission_rx)
        .unwrap();

    assert!(cancel.load(Ordering::SeqCst));
    assert_eq!(executions.load(Ordering::SeqCst), 0);
}
