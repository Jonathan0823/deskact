use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::Command,
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct FixtureDirectory(PathBuf);

impl FixtureDirectory {
    /// Create an isolated temporary workspace for one harness test.
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "deskact-task-execution-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create fixture directory");
        Self(path)
    }

    /// Return the temporary workspace path.
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for FixtureDirectory {
    /// Remove only this test's temporary workspace.
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ChatEndpoint {
    base_url: String,
    request: JoinHandle<String>,
}

impl ChatEndpoint {
    /// Start a one-request Chat Completions fixture that returns the supplied plan.
    fn responding_with(plan: &str) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture endpoint");
        listener
            .set_nonblocking(true)
            .expect("bound fixture endpoint accept");
        let address = listener.local_addr().expect("fixture endpoint address");
        let plan = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": { "content": plan }
            }]
        })
        .to_string();
        let request = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "no request reached the model fixture"
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("accept model request: {error}"),
                }
            };
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .expect("bound model fixture read");
            let request = read_http_request(&mut stream);
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                plan.len(),
                plan
            );
            stream
                .write_all(response.as_bytes())
                .expect("write fixture completion");
            request
        });
        Self {
            base_url: format!("http://{address}/v1"),
            request,
        }
    }
}

/// Read one bounded-by-the-client HTTP request through its declared body length.
fn read_http_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    loop {
        let read = stream.read(&mut buffer).expect("read fixture HTTP request");
        if read == 0 {
            break;
        }
        request.extend_from_slice(&buffer[..read]);
        if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if request.len() >= header_end + 4 + content_length {
                break;
            }
        }
    }
    String::from_utf8(request).expect("HTTP request is UTF-8")
}

/// Compile the portable native CLI fixture into the current test workspace.
/// Decode the serialized Chat Completions HTTP body captured by the loopback endpoint.
fn chat_request_body(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("HTTP request has a body separator");
    serde_json::from_str(body).expect("Chat Completions body is JSON")
}

/// Extract the user prompt from the decoded Chat Completions request.
fn user_message_content(request: &str) -> String {
    let body = chat_request_body(request);
    let messages = body["messages"].as_array().expect("messages array");
    messages
        .iter()
        .find(|message| message["role"] == "user")
        .and_then(|message| message["content"].as_str())
        .expect("user message content")
        .to_owned()
}

/// Decode the model-safe catalog candidate array embedded in the user prompt.
fn catalog_candidates_from_request(request: &str) -> Vec<serde_json::Value> {
    let content = user_message_content(request);
    let candidates = content
        .strip_prefix("Eligible catalog candidates (IDs and display names only): ")
        .and_then(|context| context.split_once("\nPropose a plan for this request:"))
        .map(|(candidates, _)| candidates)
        .expect("candidate context in user message");
    serde_json::from_str(candidates).expect("candidate context is JSON")
}

/// Search decoded JSON string values rather than escaped serialized bytes.
fn decoded_json_contains(value: &serde_json::Value, needle: &str) -> bool {
    match value {
        serde_json::Value::String(value) => value.contains(needle),
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| decoded_json_contains(value, needle)),
        serde_json::Value::Object(values) => values
            .iter()
            .any(|(key, value)| key.contains(needle) || decoded_json_contains(value, needle)),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            false
        }
    }
}

/// Compile the portable native CLI fixture into the current test workspace.
fn compile_fake_agent_desktop(directory: &FixtureDirectory) -> PathBuf {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake_agent_desktop.rs");
    let executable = directory.path().join(if cfg!(windows) {
        "agent-desktop.exe"
    } else {
        "agent-desktop"
    });
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(rustc)
        .args(["--edition=2024"])
        .arg(source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("compile cross-platform fake agent-desktop CLI");
    assert!(
        output.status.success(),
        "rustc failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    executable
}

/// Run the public task command with an isolated model endpoint and native CLI fixture.
fn run_task(
    directory: &FixtureDirectory,
    base_url: &str,
    request: &str,
    target: &std::path::Path,
    window_accessible: bool,
    window_minimized: bool,
) -> std::process::Output {
    let executable = directory.path().join(if cfg!(windows) {
        "agent-desktop.exe"
    } else {
        "agent-desktop"
    });
    Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(["task", "execute", "--request", request])
        .env("DESKACT_LLM_BASE_URL", base_url)
        .env("DESKACT_LLM_MODEL", "fixture-model")
        .env("DESKACT_AGENT_DESKTOP", &executable)
        .env("DESKACT_POWERSHELL", &executable)
        .env("DESKACT_TEST_TARGET", target)
        .env("DESKACT_TEST_ACCESSIBLE", window_accessible.to_string())
        .env("DESKACT_TEST_MINIMIZED", window_minimized.to_string())
        .env(
            "DESKACT_TEST_CALLS",
            directory.path().join("native-calls.log"),
        )
        .env("DESKACT_APP_CATALOG", directory.path().join("apps.json"))
        .output()
        .expect("run task execution harness")
}

/// Persist the single catalog entry used by the task execution fixture.
fn write_catalog(directory: &FixtureDirectory, target: &std::path::Path) {
    let store = serde_json::json!({
        "version": 1,
        "entries": [{
            "id": "calculator-id",
            "name": "Calculator",
            "target": target,
            "arguments": "",
            "working_directory": "",
            "source": "user-configured",
            "aliases": []
        }]
    });
    fs::write(
        directory.path().join("apps.json"),
        serde_json::to_vec(&store).expect("serialize fixture app catalog"),
    )
    .expect("write fixture app catalog");
}

/// Verify exact catalog-name routing uses the registered workflow without an LLM endpoint.
#[test]
fn exact_registered_open_workflow_bypasses_the_model() {
    let directory = FixtureDirectory::new();
    let target = directory.path().join("Calculator.exe");
    fs::write(&target, "fixture executable").expect("write target fixture");
    compile_fake_agent_desktop(&directory);
    write_catalog(&directory, &target);

    let output = run_task(
        &directory,
        "http://127.0.0.1:1/v1",
        "open Calculator",
        &target,
        true,
        false,
    );

    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("structured task result");
    assert_eq!(report["route"], "registered_workflow");
    assert_eq!(report["status"], "completed");
    let calls = fs::read_to_string(directory.path().join("native-calls.log"))
        .expect("native CLI invocation log");
    assert!(
        calls.lines().any(|line| line.starts_with("launch ")),
        "{calls}"
    );
}

/// Verify unverified native outcomes remain structured JSON and return a failing exit code.
#[test]
fn unresolved_task_report_returns_nonzero_status() {
    let directory = FixtureDirectory::new();
    let target = directory.path().join("Calculator.exe");
    fs::write(&target, "fixture executable").expect("write target fixture");
    compile_fake_agent_desktop(&directory);
    write_catalog(&directory, &target);

    let output = run_task(
        &directory,
        "http://127.0.0.1:1/v1",
        "open Calculator",
        &target,
        false,
        false,
    );

    assert!(!output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("structured failure report on stdout");
    assert_eq!(report["status"], "failed");
    assert_eq!(report["achieved_postconditions"], serde_json::json!([]));
    assert_eq!(report["unresolved_steps"].as_array().unwrap().len(), 1);
}

/// Verify compound requests are rejected before making a model request or native call.
#[test]
fn compound_request_is_rejected_before_model_planning() {
    let directory = FixtureDirectory::new();
    let target = directory.path().join("Calculator.exe");
    fs::write(&target, "fixture executable").expect("write target fixture");
    compile_fake_agent_desktop(&directory);
    write_catalog(&directory, &target);

    let output = run_task(
        &directory,
        "http://127.0.0.1:1/v1",
        "open Calculator and type 123",
        &target,
        true,
        false,
    );

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("compound or other requests are unsupported")
    );
    let calls = fs::read_to_string(directory.path().join("native-calls.log"))
        .expect("native CLI invocation log");
    assert!(!calls.lines().any(|line| line == "version"), "{calls}");
}

/// Keep the manual restore prompt on stderr so stdout remains parseable JSON.
#[test]
fn minimized_window_prompt_does_not_pollute_json_stdout() {
    let directory = FixtureDirectory::new();
    let target = directory.path().join("Calculator.exe");
    fs::write(&target, "fixture executable").expect("write target fixture");
    compile_fake_agent_desktop(&directory);
    write_catalog(&directory, &target);

    let output = run_task(
        &directory,
        "http://127.0.0.1:1/v1",
        "open Calculator",
        &target,
        true,
        true,
    );

    assert!(!output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("structured task report on stdout");
    assert_eq!(report["status"], "failed");
    assert!(String::from_utf8_lossy(&output.stderr).contains("is minimized"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("Restore it manually"));
}

/// Verify a later invalid step prevents an earlier valid app-opening step from launching.
#[test]
fn unsupported_model_operation_fails_before_any_native_mutation() {
    let directory = FixtureDirectory::new();
    let target = directory.path().join("Calculator.exe");
    fs::write(&target, "fixture executable").expect("write target fixture");
    compile_fake_agent_desktop(&directory);
    write_catalog(&directory, &target);
    let plan = r#"{"version":1,"summary":"Open then click","steps":[{"operation":"open_app","parameters":{"app_id":"calculator-id"},"expected_outcome":"Calculator is open"},{"operation":"click","parameters":{"app_id":"calculator-id"},"expected_outcome":"Calculator is open"}]}"#;
    let endpoint = ChatEndpoint::responding_with(plan);

    let output = run_task(
        &directory,
        &endpoint.base_url,
        "open my calculator",
        &target,
        true,
        false,
    );
    let _model_request = endpoint.request.join().expect("model fixture thread");

    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("step 2 uses unsupported operation `click`"),
        "{error}"
    );
    let calls = fs::read_to_string(directory.path().join("native-calls.log"))
        .expect("native CLI invocation log");
    assert!(
        !calls
            .lines()
            .any(|line| line == "version" || line.starts_with("launch ")),
        "{calls}"
    );
}

/// Verify model-grounded app selection passes through native resolution and evidence reporting.
#[test]
fn unknown_request_uses_model_candidates_then_verified_native_resolution() {
    let directory = FixtureDirectory::new();
    let target = directory.path().join("Calculator.exe");
    fs::write(&target, "fixture executable").expect("write target fixture");
    compile_fake_agent_desktop(&directory);
    write_catalog(&directory, &target);
    let plan = r#"{"version":1,"summary":"Open Calculator","steps":[{"operation":"open_app","parameters":{"app_id":"calculator-id"},"expected_outcome":"Calculator is open"}]}"#;
    let endpoint = ChatEndpoint::responding_with(plan);

    let output = run_task(
        &directory,
        &endpoint.base_url,
        "open my calculator",
        &target,
        true,
        false,
    );
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let model_request = endpoint.request.join().expect("model fixture thread");
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("structured task result");
    assert_eq!(report["route"], "primary_model");
    assert_eq!(report["status"], "completed");
    assert_eq!(report["summary"], "Opened Calculator");
    assert_eq!(report["unresolved_steps"], serde_json::json!([]));
    assert_eq!(
        report["achieved_postconditions"][0],
        "Calculator is open in one uniquely matched accessible window with a fresh complete snapshot"
    );
    let request_body = chat_request_body(&model_request);
    assert!(
        request_body["messages"][0]["content"]
            .as_str()
            .unwrap()
            .contains(
                "expected_outcome must be exactly `<selected candidate display_name> is open`"
            )
    );
    let candidates = catalog_candidates_from_request(&model_request);
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["id"], "calculator-id");
    assert_eq!(candidates[0]["display_name"], "Calculator");
    assert!(candidates[0].get("target").is_none());
    assert!(!decoded_json_contains(
        &serde_json::Value::Array(candidates),
        target.to_str().unwrap()
    ));

    let calls = fs::read_to_string(directory.path().join("native-calls.log"))
        .expect("native CLI invocation log");
    assert!(calls.lines().any(|line| line == "version"), "{calls}");
    assert!(
        calls.lines().any(|line| line.starts_with("launch ")),
        "{calls}"
    );
    assert!(calls.contains(target.to_str().unwrap()), "{calls}");
    assert!(
        calls.lines().any(|line| line.starts_with("list-windows")),
        "{calls}"
    );
    assert!(
        calls.lines().any(|line| line.starts_with("snapshot ")),
        "{calls}"
    );
}
