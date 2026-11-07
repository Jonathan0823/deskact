use std::process::{Command, Output};

/// Run one public approval-policy simulation through the developer harness.
fn run_scenario(scenario: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(["task", "approval-simulate", "--scenario", scenario])
        .output()
        .expect("run approval simulation harness")
}

/// Decode the structured report emitted by the public simulation command.
fn report(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("structured approval report")
}

/// Model risk claims cannot suppress policy classification or dispatch without explicit approval.
#[test]
fn consequential_action_waits_for_approval_despite_model_claim() {
    let output = run_scenario("unapproved");
    let report = report(&output);

    assert_eq!(report["risk_class"], "consequential");
    assert_eq!(report["model_risk_claim"], "read_only");
    assert_eq!(report["status"], "awaiting_approval");
    assert_eq!(report["action_attempts"], 0);
    assert_eq!(report["scope"]["action"], "delete_file");
    assert_eq!(report["scope"]["target"], "fixture://documents/report.txt");
    assert_eq!(report["scope"]["parameters"]["permanent"], true);
}

/// An exact explicit approval reaches only the non-destructive recording adapter.
#[test]
fn explicit_approval_dispatches_the_exact_scoped_action() {
    let output = run_scenario("approved");
    let report = report(&output);

    assert_eq!(report["risk_class"], "consequential");
    assert_eq!(report["status"], "executed");
    assert_eq!(report["action_attempts"], 1);
    assert_eq!(report["executed_scope"]["action"], "delete_file");
    assert_eq!(
        report["executed_scope"]["target"],
        "fixture://documents/report.txt"
    );
    assert_eq!(report["executed_scope"]["parameters"]["permanent"], true);
}

/// A grant cannot authorize a different action, target, or parameter set.
#[test]
fn any_scope_change_invalidates_prior_approval_without_dispatch() {
    for scenario in ["stale-action", "stale-target", "stale-parameters"] {
        let output = run_scenario(scenario);
        let report = report(&output);

        assert_eq!(report["risk_class"], "consequential", "{scenario}");
        assert_eq!(report["status"], "approval_stale", "{scenario}");
        assert_eq!(report["approval_valid"], false, "{scenario}");
        assert_eq!(report["action_attempts"], 0, "{scenario}");
        assert_ne!(
            report["approved_scope"], report["current_scope"],
            "{scenario}"
        );
        match scenario {
            "stale-action" => assert_ne!(
                report["approved_scope"]["action"],
                report["current_scope"]["action"]
            ),
            "stale-target" => assert_ne!(
                report["approved_scope"]["target"],
                report["current_scope"]["target"]
            ),
            "stale-parameters" => assert_ne!(
                report["approved_scope"]["parameters"],
                report["current_scope"]["parameters"]
            ),
            _ => unreachable!("known stale scope scenario"),
        }
    }
}
