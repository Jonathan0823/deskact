use std::process::Command;

fn simulate(scenario: &str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(["task", "simulate", "--scenario", scenario])
        .output()
        .expect("run task simulation")
}

#[test]
fn stale_references_require_fresh_observation_before_a_dependent_action() {
    let output = simulate("stale-reference");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task simulation reports structured JSON");
    let events = report["events"].as_array().expect("ordered task events");
    let fresh = events
        .iter()
        .position(|event| event == "fresh_observation")
        .expect("fresh observation event");
    let identified = events
        .iter()
        .position(|event| event == "reference_reidentified")
        .expect("reference re-identification event");
    let dependent = events
        .iter()
        .position(|event| event == "dependent_action_started")
        .expect("dependent action event");
    assert!(
        fresh < identified && identified < dependent,
        "events: {events:?}"
    );
    assert_eq!(report["status"], "completed");
}

#[test]
fn incomplete_observations_are_refreshed_before_reidentifying_references() {
    let output = simulate("incomplete-observation");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task simulation reports structured JSON");
    let events = report["events"].as_array().expect("ordered task events");
    let fresh = events
        .iter()
        .position(|event| event == "fresh_observation")
        .expect("fresh observation event");
    let identified = events
        .iter()
        .position(|event| event == "reference_reidentified")
        .expect("reference re-identification event");
    assert!(fresh < identified, "events: {events:?}");
}

#[test]
fn permission_failure_stops_without_retrying_the_tool_call() {
    let output = simulate("permission-denied");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task simulation reports structured JSON");
    assert_eq!(report["status"], "failed");
    assert_eq!(report["mutation_attempts"], 1);
    assert_eq!(report["retry_count"], 0);
    assert_eq!(report["reason"], "permission_denied");
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("grant the required access")
    );
}

#[test]
fn a_second_mutating_request_is_reported_busy_and_not_queued() {
    let output = simulate("busy-request");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task simulation reports structured JSON");
    assert_eq!(report["status"], "busy");
    assert_eq!(report["mutation_attempts"], 1);
    assert_eq!(report["queued_requests"], 0);
    assert!(report["message"].as_str().unwrap().contains("not queued"));
    assert!(
        report["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event == "request_rejected_busy")
    );
}

#[test]
fn cancellation_stops_new_steps_and_reports_in_flight_uncertainty() {
    let output = simulate("cancel-in-flight");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task simulation reports structured JSON");
    assert_eq!(report["status"], "cancelled");
    assert_eq!(report["mutation_attempts"], 1);
    assert_eq!(report["delivery_disposition"], "unknown");
    assert_eq!(report["unstarted_steps"], 1);
    assert!(
        report["message"]
            .as_str()
            .unwrap()
            .contains("in-flight delivery is unknown")
    );
}

#[test]
fn unclear_ui_interference_requires_user_input_without_resuming() {
    let output = simulate("ui-interference-unclear");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task simulation reports structured JSON");
    let events = report["events"].as_array().expect("ordered task events");
    assert_eq!(report["status"], "partial");
    assert!(events.iter().any(|event| event == "user_input_required"));
    assert!(!events.iter().any(|event| event == "task_resumed"));
}

#[test]
fn ui_interference_revalidates_observations_and_plan_before_resuming() {
    let output = simulate("ui-interference");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task simulation reports structured JSON");
    let events = report["events"].as_array().expect("ordered task events");
    let pause = events
        .iter()
        .position(|event| event == "task_paused")
        .unwrap();
    let observation = events
        .iter()
        .position(|event| event == "fresh_observation")
        .unwrap();
    let validation = events
        .iter()
        .position(|event| event == "plan_revalidated")
        .unwrap();
    let resume = events
        .iter()
        .position(|event| event == "task_resumed")
        .unwrap();
    assert!(
        pause < observation && observation < validation && validation < resume,
        "events: {events:?}"
    );
    assert_eq!(report["status"], "completed");
}

#[test]
fn observed_postcondition_prevents_repeating_an_already_delivered_mutation() {
    let output = simulate("unknown-delivery-achieved");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task simulation reports structured JSON");
    assert_eq!(report["status"], "completed");
    assert_eq!(report["mutation_attempts"], 1);
    assert_eq!(report["retry_count"], 0);
}

#[test]
fn safe_retry_is_bounded_and_requires_negative_post_state_evidence() {
    let output = simulate("unknown-delivery-safe-retry");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task simulation reports structured JSON");
    assert_eq!(report["status"], "completed");
    assert_eq!(report["mutation_attempts"], 2);
    assert_eq!(report["post_state_observations"], 1);
    assert_eq!(report["retry_count"], 1);
}

#[test]
fn unknown_mutation_delivery_is_observed_before_any_retry() {
    let output = simulate("unknown-delivery");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("task simulation reports structured JSON");
    assert_eq!(report["status"], "partial");
    assert_eq!(report["mutation_attempts"], 1);
    assert_eq!(report["post_state_observations"], 1);
    assert_eq!(report["retry_count"], 0);
    assert_eq!(report["delivery_disposition"], "unknown");
    let events = report["events"].as_array().expect("ordered task events");
    let observed = events
        .iter()
        .position(|event| event == "post_state_observed")
        .expect("post-state observation event");
    let retry_decision = events
        .iter()
        .position(|event| event == "retry_suppressed")
        .expect("retry decision event");
    assert!(observed < retry_decision, "events: {events:?}");
}
