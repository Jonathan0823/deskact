use std::{
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    process::{Command, Output},
    thread::JoinHandle,
    time::{Duration, Instant},
};

fn run_llm(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(arguments)
        .output()
        .expect("run developer harness")
}

struct LoopbackEndpoint {
    base_url: String,
    request: JoinHandle<String>,
}

impl LoopbackEndpoint {
    fn respond(status: u16, reason: &str, response_body: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fixture endpoint");
        let address = listener.local_addr().expect("fixture endpoint address");
        listener
            .set_nonblocking(true)
            .expect("set fixture listener nonblocking");
        let response_body = response_body.to_owned();
        let reason = reason.to_owned();
        let request = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(error)
                        if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("accept fixture request: {error}"),
                }
            };
            let request = read_http_request(&mut stream);
            write!(
                stream,
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response_body}",
                response_body.len()
            )
            .expect("write fixture response");
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
    let mut buffer = [0; 4096];
    loop {
        let bytes_read = stream.read(&mut buffer).expect("read fixture request");
        assert_ne!(
            bytes_read, 0,
            "fixture endpoint received a complete request"
        );
        request.extend_from_slice(&buffer[..bytes_read]);
        let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header = String::from_utf8_lossy(&request[..header_end]);
        let content_length = header
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().expect("valid content length"))
            })
            .unwrap_or(0);
        if request.len() >= header_end + 4 + content_length {
            break;
        }
    }
    String::from_utf8(request).expect("fixture request is UTF-8")
}

#[test]
fn llm_test_sends_a_synthetic_chat_completion_and_reports_its_limitations() {
    let endpoint = LoopbackEndpoint::respond(
        200,
        "OK",
        r#"{"choices":[{"message":{"content":"DeskAct connection check succeeded"}}]}"#,
    );
    let output = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(["llm", "test"])
        .env("DESKACT_LLM_BASE_URL", &endpoint.base_url)
        .env("DESKACT_LLM_MODEL", "fixture-model")
        .env("DESKACT_LLM_API_KEY", "fixture-secret-never-print")
        .output()
        .expect("run developer harness");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "stdout: {stdout}; stderr: {stderr}"
    );
    let request = endpoint.request.join().expect("fixture request thread");
    assert!(stdout.contains("endpoint reachable"), "{stdout}");
    assert!(stdout.contains("authentication accepted"), "{stdout}");
    assert!(stdout.contains("response content received"), "{stdout}");
    assert!(
        stdout.contains("does not guarantee action-plan compatibility"),
        "{stdout}"
    );
    assert!(!stdout.contains("fixture-secret-never-print"), "{stdout}");
    assert!(
        request
            .to_ascii_lowercase()
            .contains("authorization: bearer fixture-secret-never-print"),
        "{request}"
    );
    assert!(request.contains("fixture-model"), "{request}");
    assert!(request.contains("synthetic"), "{request}");
    assert!(!request.contains("user-request"), "{request}");
}

#[test]
fn llm_test_rejects_remote_http_before_network_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(["llm", "test"])
        .env("DESKACT_LLM_BASE_URL", "http://example.com/v1")
        .env("DESKACT_LLM_MODEL", "fixture-model")
        .output()
        .expect("run developer harness");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(
        stderr.contains("Use HTTPS for remote LLM endpoints"),
        "{stderr}"
    );
}

#[test]
fn llm_test_reports_endpoint_reachability_and_authentication_failure_separately() {
    let endpoint = LoopbackEndpoint::respond(401, "Unauthorized", r#"{"error":"hidden"}"#);
    let output = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(["llm", "test"])
        .env("DESKACT_LLM_BASE_URL", &endpoint.base_url)
        .env("DESKACT_LLM_MODEL", "fixture-model")
        .env("DESKACT_LLM_API_KEY", "fixture-secret-never-print")
        .output()
        .expect("run developer harness");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let request = endpoint.request.join().expect("fixture request thread");

    assert!(!output.status.success());
    assert!(
        stderr.contains("Endpoint reachable, authentication rejected (HTTP 401)"),
        "{stderr}"
    );
    assert!(!stderr.contains("fixture-secret-never-print"), "{stderr}");
    assert!(
        !stderr.contains("hidden"),
        "provider error bodies are not exposed"
    );
    assert!(request.contains("synthetic"), "{request}");
}

#[test]
fn llm_validate_plan_rejects_a_plan_without_steps() {
    let plan = r#"{"version":1,"summary":"Open Calculator","steps":[]}"#;
    let output = run_llm(&["llm", "validate-plan", "--response-json", plan]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(
        stderr.contains("Action plan must contain between 1 and 20 steps"),
        "{stderr}"
    );
}

#[test]
fn llm_validate_plan_accepts_a_complete_versioned_plan() {
    let plan = r#"{"version":1,"summary":"Open Calculator","steps":[{"operation":"open_app","parameters":{"app_id":"calculator-id"},"expected_outcome":"Calculator is open"}]}"#;
    let output = run_llm(&["llm", "validate-plan", "--response-json", plan]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "stdout: {stdout}; stderr: {stderr}"
    );
    assert!(
        stdout.contains("Validated action plan version 1 with 1 step"),
        "{stdout}"
    );
}
