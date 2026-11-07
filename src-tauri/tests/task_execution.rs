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

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct ChatEndpoint {
    base_url: String,
    request: JoinHandle<String>,
}

impl ChatEndpoint {
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

fn run_task(
    directory: &FixtureDirectory,
    base_url: &str,
    request: &str,
    target: &std::path::Path,
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
        .env(
            "DESKACT_TEST_CALLS",
            directory.path().join("native-calls.log"),
        )
        .env("DESKACT_APP_CATALOG", directory.path().join("apps.json"))
        .output()
        .expect("run task execution harness")
}

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

#[test]
fn unsupported_model_operation_fails_before_any_native_mutation() {
    let directory = FixtureDirectory::new();
    let target = directory.path().join("Calculator.exe");
    fs::write(&target, "fixture executable").expect("write target fixture");
    compile_fake_agent_desktop(&directory);
    write_catalog(&directory, &target);
    let plan = r#"{"version":1,"summary":"Click a button","steps":[{"operation":"click","parameters":{"app_id":"calculator-id"},"expected_outcome":"Calculator is open"}]}"#;
    let endpoint = ChatEndpoint::responding_with(plan);

    let output = run_task(
        &directory,
        &endpoint.base_url,
        "open my calculator",
        &target,
    );
    let _model_request = endpoint.request.join().expect("model fixture thread");

    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("unsupported operation `click`"), "{error}");
    let calls = fs::read_to_string(directory.path().join("native-calls.log"))
        .expect("native CLI invocation log");
    assert!(
        !calls
            .lines()
            .any(|line| line == "version" || line.starts_with("launch ")),
        "{calls}"
    );
}

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
    assert!(model_request.contains("calculator-id"), "{model_request}");
    assert!(model_request.contains("Calculator"), "{model_request}");
    assert!(
        !model_request.contains(target.to_str().unwrap()),
        "{model_request}"
    );

    let calls = fs::read_to_string(directory.path().join("native-calls.log"))
        .expect("native CLI invocation log");
    assert!(calls.lines().any(|line| line == "version"), "{calls}");
    assert!(
        calls.lines().any(|line| line.starts_with("launch ")),
        "{calls}"
    );
    assert!(
        calls.lines().any(|line| line.starts_with("list-windows")),
        "{calls}"
    );
    assert!(
        calls.lines().any(|line| line.starts_with("snapshot ")),
        "{calls}"
    );
}
