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
    requests: JoinHandle<Vec<String>>,
}

impl LoopbackEndpoint {
    fn respond(status: u16, reason: &str, response_body: &str) -> Self {
        Self::respond_many(vec![(status, reason.to_owned(), response_body.to_owned())])
    }

    fn respond_many(responses: Vec<(u16, String, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback fixture endpoint");
        let address = listener.local_addr().expect("fixture endpoint address");
        listener
            .set_nonblocking(true)
            .expect("set fixture listener nonblocking");
        let requests = std::thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, reason, response_body) in responses {
                let deadline = Instant::now() + Duration::from_secs(2);
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(connection) => break connection,
                        Err(error)
                            if error.kind() == ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
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
                requests.push(request);
            }
            requests
        });
        Self {
            base_url: format!("http://{address}/v1"),
            requests,
        }
    }
}

fn read_http_request(stream: &mut TcpStream) -> String {
    stream
        .set_nonblocking(false)
        .expect("set fixture connection blocking");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("bound fixture request read");
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
    let requests = endpoint.requests.join().expect("fixture request thread");
    let request = requests.into_iter().next().expect("one fixture request");
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
    let requests = endpoint.requests.join().expect("fixture request thread");
    let request = requests.into_iter().next().expect("one fixture request");

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
fn llm_plan_requests_one_correction_for_an_invalid_plan_then_returns_validated_json() {
    let invalid_plan = r#"{"version":1,"summary":"Open Calculator","steps":[]}"#;
    let valid_plan = r#"{"version":1,"summary":"Open Calculator","steps":[{"operation":"open_app","parameters":{"app_id":"calculator-id"},"expected_outcome":"Calculator is open"}]}"#;
    let response = |content: &str| {
        serde_json::json!({"choices":[{"finish_reason":"stop","message":{"content":content}}]})
            .to_string()
    };
    let endpoint = LoopbackEndpoint::respond_many(vec![
        (200, "OK".into(), response(invalid_plan)),
        (200, "OK".into(), response(valid_plan)),
    ]);
    let output = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(["llm", "plan", "Open Calculator"])
        .env("DESKACT_LLM_BASE_URL", &endpoint.base_url)
        .env("DESKACT_LLM_MODEL", "fixture-model")
        .env("DESKACT_LLM_API_KEY", "fixture-secret-never-print")
        .output()
        .expect("run developer harness");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let requests = endpoint.requests.join().expect("fixture request thread");

    assert!(
        output.status.success(),
        "stdout: {stdout}; stderr: {stderr}"
    );
    let returned_plan: serde_json::Value =
        serde_json::from_str(&stdout).expect("CLI returns validated plan JSON");
    assert_eq!(returned_plan["version"], 1);
    assert_eq!(returned_plan["steps"].as_array().unwrap().len(), 1);
    assert!(!stdout.contains("fixture-secret-never-print"), "{stdout}");
    assert_eq!(
        requests.len(),
        2,
        "exactly one bounded correction is allowed"
    );
    assert!(requests[0].contains("Open Calculator"), "{}", requests[0]);
    let correction_body = requests[1]
        .split_once("\r\n\r\n")
        .expect("correction HTTP body")
        .1;
    let correction_json: serde_json::Value =
        serde_json::from_str(correction_body).expect("correction Chat Completions request");
    let messages = correction_json["messages"]
        .as_array()
        .expect("messages array");
    assert_eq!(messages.len(), 4, "{correction_json}");
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(messages[1]["role"], "user");
    assert!(
        messages[1]["content"]
            .as_str()
            .unwrap()
            .contains("Open Calculator")
    );
    assert_eq!(messages[2]["role"], "assistant");
    assert_eq!(messages[3]["role"], "user");
    assert!(
        messages[3]["content"]
            .as_str()
            .unwrap()
            .contains("validation")
    );
    assert!(requests.iter().all(|request| {
        !request
            .split_once("\r\n\r\n")
            .expect("HTTP request body separator")
            .1
            .contains("fixture-secret-never-print")
    }));
}

#[test]
fn llm_plan_rejects_length_and_content_filter_finish_reasons() {
    let plan = r#"{"version":1,"summary":"Plan","steps":[{"operation":"open_app","parameters":{},"expected_outcome":"App is open"}]}"#;
    for (finish_reason, diagnostic) in [
        ("length", "completion was cut off by the token limit"),
        (
            "content_filter",
            "completion was rejected by the provider content filter",
        ),
    ] {
        let response = serde_json::json!({"choices":[{"finish_reason":finish_reason,"message":{"content":plan}}]}).to_string();
        let endpoint = LoopbackEndpoint::respond(200, "OK", &response);
        let output = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .args(["llm", "plan", "Open an app"])
            .env("DESKACT_LLM_BASE_URL", &endpoint.base_url)
            .env("DESKACT_LLM_MODEL", "fixture-model")
            .output()
            .expect("run developer harness");
        let stderr = String::from_utf8_lossy(&output.stderr);
        let requests = endpoint.requests.join().expect("fixture request thread");

        assert!(!output.status.success());
        assert!(stderr.to_ascii_lowercase().contains(diagnostic), "{stderr}");
        assert_eq!(requests.len(), 1, "incomplete completions are not retried");
    }
}

#[test]
fn llm_plan_suppresses_a_valid_plan_that_echoes_the_configured_credential() {
    let secret = "fixture-secret-never-print";
    let plan = format!(
        r#"{{"version":1,"summary":"Plan","steps":[{{"operation":"open_app","parameters":{{"echo":"{secret}"}},"expected_outcome":"App is open"}}]}}"#
    );
    let response = serde_json::json!({"choices":[{"message":{"content":plan}}]}).to_string();
    let endpoint = LoopbackEndpoint::respond(200, "OK", &response);
    let output = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(["llm", "plan", "Open an app"])
        .env("DESKACT_LLM_BASE_URL", &endpoint.base_url)
        .env("DESKACT_LLM_MODEL", "fixture-model")
        .env("DESKACT_LLM_API_KEY", secret)
        .output()
        .expect("run developer harness");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let requests = endpoint.requests.join().expect("fixture request thread");

    assert!(!output.status.success());
    assert!(
        stderr.contains("plan contains the configured credential and was suppressed"),
        "{stderr}"
    );
    assert!(!stdout.contains(secret), "{stdout}");
    assert!(!stderr.contains(secret), "{stderr}");
    assert_eq!(
        requests.len(),
        1,
        "a credential echo is not corrected or retried"
    );
}

#[test]
fn llm_plan_does_not_expose_credentials_in_malformed_version_errors() {
    let secret = "fixture-secret-never-print";
    let valid_plan = r#"{"version":1,"summary":"Open Calculator","steps":[{"operation":"open_app","parameters":{"app_id":"calculator-id"},"expected_outcome":"Calculator is open"}]}"#;
    let initial_with_secret = format!(
        r#"{{"version":"{secret}","summary":"Open Calculator","steps":[{{"operation":"open_app","parameters":{{}},"expected_outcome":"Calculator is open"}}]}}"#
    );
    let corrected_with_secret = format!(
        r#"{{"version":"{secret}","summary":"Open Calculator","steps":[{{"operation":"open_app","parameters":{{}},"expected_outcome":"Calculator is open"}}]}}"#
    );
    let empty_plan = r#"{"version":1,"summary":"Open Calculator","steps":[]}"#;
    let response = |content: &str| {
        serde_json::json!({"choices":[{"finish_reason":"stop","message":{"content":content}}]})
            .to_string()
    };
    let scenarios = [
        (response(&initial_with_secret), response(valid_plan), true),
        (
            response(empty_plan),
            response(&corrected_with_secret),
            false,
        ),
    ];

    for (initial, corrected, should_succeed) in scenarios {
        let endpoint = LoopbackEndpoint::respond_many(vec![
            (200, "OK".into(), initial),
            (200, "OK".into(), corrected),
        ]);
        let output = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .args(["llm", "plan", "Open Calculator"])
            .env("DESKACT_LLM_BASE_URL", &endpoint.base_url)
            .env("DESKACT_LLM_MODEL", "fixture-model")
            .env("DESKACT_LLM_API_KEY", secret)
            .output()
            .expect("run developer harness");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let requests = endpoint.requests.join().expect("fixture request thread");

        assert_eq!(
            output.status.success(),
            should_succeed,
            "{stdout}; {stderr}"
        );
        assert!(!stdout.contains(secret), "{stdout}");
        assert!(!stderr.contains(secret), "{stderr}");
        assert_eq!(requests.len(), 2);
    }
}

#[test]
fn llm_plan_fails_after_one_invalid_correction() {
    let invalid_plan = r#"{"version":1,"summary":"Open Calculator","steps":[]}"#;
    let response =
        serde_json::json!({"choices":[{"message":{"content":invalid_plan}}]}).to_string();
    let endpoint = LoopbackEndpoint::respond_many(vec![
        (200, "OK".into(), response.clone()),
        (200, "OK".into(), response),
    ]);
    let output = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(["llm", "plan", "Open Calculator"])
        .env("DESKACT_LLM_BASE_URL", &endpoint.base_url)
        .env("DESKACT_LLM_MODEL", "fixture-model")
        .output()
        .expect("run developer harness");
    let stderr = String::from_utf8_lossy(&output.stderr);
    let requests = endpoint.requests.join().expect("fixture request thread");

    assert!(!output.status.success());
    assert!(
        stderr.contains("after one bounded correction attempt"),
        "{stderr}"
    );
    assert_eq!(requests.len(), 2, "no third request may be sent");
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
