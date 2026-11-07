use std::process::{Command, Output};

fn run_llm(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(arguments)
        .output()
        .expect("run developer harness")
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
