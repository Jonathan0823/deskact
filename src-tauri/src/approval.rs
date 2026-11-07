//! Action classification and exact-scope approval policy.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Value, json};

use crate::DeskActError;

/// Risk classification derived locally from an action identifier, never from model claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionRiskClass {
    /// An observation that does not change local or external state.
    ReadOnly,
    /// A reversible local action that does not require explicit approval in this policy slice.
    Reversible,
    /// A destructive, externally visible, or material system-setting action.
    Consequential,
}

/// Exact action identity shown to the user and retained for later authorization checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionScope {
    /// Locally interpreted operation identifier.
    pub action: String,
    /// Exact target identity for the proposed operation.
    pub target: String,
    /// Exact validated parameters for the proposed operation.
    pub parameters: BTreeMap<String, Value>,
}

/// Consequential action presented to the user for an explicit approval decision.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct ApprovalRequest {
    /// Exact action scope that must be displayed before approval.
    pub scope: ActionScope,
    /// Locally determined risk classification.
    pub risk_class: ActionRiskClass,
}

/// One-use authorization issued only after the caller receives an explicit user approval.
#[derive(Debug, PartialEq, Eq)]
pub struct ApprovalGrant {
    scope: ActionScope,
}

/// Outcome of one non-destructive action-specific approval simulation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApprovalSimulationReport {
    /// Simulation lifecycle status.
    pub status: String,
    /// Rust policy classification, independent of the model claim.
    pub risk_class: ActionRiskClass,
    /// Untrusted risk claim carried by the fixture proposal.
    pub model_risk_claim: String,
    /// Proposed scope when awaiting approval.
    pub scope: Option<ActionScope>,
    /// Scope the fixture user explicitly approved, if any.
    pub approved_scope: Option<ActionScope>,
    /// Scope evaluated immediately before dispatch.
    pub current_scope: ActionScope,
    /// Whether the approval remains valid for the current scope.
    pub approval_valid: Option<bool>,
    /// Number of calls observed by the non-destructive recording adapter.
    pub action_attempts: usize,
    /// Scope recorded by the adapter after valid authorization.
    pub executed_scope: Option<ActionScope>,
}

/// Classify an action from its locally interpreted operation, never from model claims.
pub fn classify_action(scope: &ActionScope) -> Result<ActionRiskClass, DeskActError> {
    if scope.target.trim().is_empty() || scope.target.chars().any(char::is_control) {
        return Err(DeskActError::new(
            "Action scope must contain a non-empty target without control characters".into(),
        ));
    }
    match scope.action.as_str() {
        "read_file" => Ok(ActionRiskClass::ReadOnly),
        "open_app" => Ok(ActionRiskClass::Reversible),
        "delete_file"
        | "overwrite_file"
        | "send_message"
        | "purchase"
        | "install_app"
        | "change_system_setting" => Ok(ActionRiskClass::Consequential),
        _ => Err(DeskActError::new(format!(
            "Action `{}` has no local risk classification and is denied",
            scope.action
        ))),
    }
}

/// Authorization outcome from the risk policy before action-adapter dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// The action is read-only/reversible or has an exact matching approval scope.
    Allowed,
    /// A consequential action has not received an explicit approval for its scope.
    RequiresApproval,
    /// The stored approval refers to different action details and cannot be reused.
    StaleApproval,
}

/// Create an approval request only for locally classified consequential actions.
pub fn request_approval(scope: &ActionScope) -> Result<Option<ApprovalRequest>, DeskActError> {
    let risk_class = classify_action(scope)?;
    Ok(
        (risk_class == ActionRiskClass::Consequential).then(|| ApprovalRequest {
            scope: scope.clone(),
            risk_class,
        }),
    )
}

/// Convert a displayed request into a one-use grant after an explicit user approval event.
pub fn explicitly_approve(request: ApprovalRequest) -> ApprovalGrant {
    ApprovalGrant {
        scope: request.scope,
    }
}

/// Re-evaluate risk and compare the approval scope immediately before dispatch.
pub fn authorize_action(
    current: &ActionScope,
    approved: Option<ApprovalGrant>,
) -> Result<ApprovalDecision, DeskActError> {
    let risk_class = classify_action(current)?;
    if let Some(approved) = approved {
        return Ok(if approved.scope == *current {
            ApprovalDecision::Allowed
        } else {
            ApprovalDecision::StaleApproval
        });
    }
    Ok(if risk_class == ActionRiskClass::Consequential {
        ApprovalDecision::RequiresApproval
    } else {
        ApprovalDecision::Allowed
    })
}

/// Adapter for one authorized action; the simulation uses a recorder and never mutates the OS.
trait ActionAdapter {
    /// Record or perform one action after the approval policy permits dispatch.
    fn execute(&mut self, scope: &ActionScope);
}

/// Test adapter that records authorized scopes without changing external state.
#[derive(Default)]
struct RecordingActionAdapter {
    executed: Vec<ActionScope>,
}

impl ActionAdapter for RecordingActionAdapter {
    fn execute(&mut self, scope: &ActionScope) {
        self.executed.push(scope.clone());
    }
}

/// Run a deterministic approval scenario through the same policy check used before adapter dispatch.
pub fn simulate_approval_scenario(
    scenario: &str,
) -> Result<ApprovalSimulationReport, DeskActError> {
    if !matches!(
        scenario,
        "unapproved" | "approved" | "stale-action" | "stale-target" | "stale-parameters"
    ) {
        return Err(DeskActError::new(format!(
            "Unknown approval simulation scenario `{scenario}`"
        )));
    }

    let original = fixture_scope();
    let approval_request = request_approval(&original)?
        .ok_or_else(|| DeskActError::new("Fixture action should require approval".into()))?;
    let risk_class = approval_request.risk_class;
    let grant = (scenario != "unapproved").then(|| explicitly_approve(approval_request));
    let approved_scope = grant.as_ref().map(|grant| grant.scope.clone());
    let mut current_scope = original.clone();
    match scenario {
        "stale-action" => current_scope.action = "overwrite_file".into(),
        "stale-target" => current_scope.target = "fixture://documents/other-report.txt".into(),
        "stale-parameters" => {
            current_scope
                .parameters
                .insert("permanent".into(), json!(false));
        }
        "unapproved" | "approved" => {}
        _ => unreachable!("scenario was checked above"),
    }

    let mut adapter = RecordingActionAdapter::default();
    let authorization = authorize_action(&current_scope, grant)?;
    let status = match authorization {
        ApprovalDecision::Allowed => {
            adapter.execute(&current_scope);
            "executed"
        }
        ApprovalDecision::RequiresApproval => "awaiting_approval",
        ApprovalDecision::StaleApproval => "approval_stale",
    };
    let approval_valid = match authorization {
        ApprovalDecision::Allowed if approved_scope.is_some() => Some(true),
        ApprovalDecision::StaleApproval => Some(false),
        ApprovalDecision::Allowed | ApprovalDecision::RequiresApproval => None,
    };

    Ok(ApprovalSimulationReport {
        status: status.into(),
        risk_class,
        model_risk_claim: "read_only".into(),
        scope: (authorization == ApprovalDecision::RequiresApproval).then_some(original),
        approved_scope,
        current_scope,
        approval_valid,
        action_attempts: adapter.executed.len(),
        executed_scope: adapter.executed.into_iter().next(),
    })
}

/// Build the fixed destructive-action scope used only by the non-destructive simulation.
fn fixture_scope() -> ActionScope {
    ActionScope {
        action: "delete_file".into(),
        target: "fixture://documents/report.txt".into(),
        parameters: BTreeMap::from([
            ("permanent".into(), json!(true)),
            ("reason".into(), json!("user requested deletion")),
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Risk classification depends on the locally recognized action, not the model's claim.
    #[test]
    fn action_classification_is_local_and_unknown_operations_fail_closed() {
        assert_eq!(
            classify_action(&fixture_scope()).unwrap(),
            ActionRiskClass::Consequential
        );
        let reversible = ActionScope {
            action: "open_app".into(),
            ..fixture_scope()
        };
        assert_eq!(
            classify_action(&reversible).unwrap(),
            ActionRiskClass::Reversible
        );
        assert!(request_approval(&reversible).unwrap().is_none());
        assert_eq!(
            authorize_action(&reversible, None).unwrap(),
            ApprovalDecision::Allowed
        );
        let unknown = ActionScope {
            action: "model_invented_action".into(),
            ..fixture_scope()
        };
        assert!(classify_action(&unknown).is_err());
        let incomplete = ActionScope {
            target: "  ".into(),
            ..fixture_scope()
        };
        assert!(classify_action(&incomplete).is_err());
    }

    /// An approval authorizes an exact scope and becomes stale after any scope change.
    #[test]
    fn authorization_requires_exact_scope_match() {
        let scope = fixture_scope();
        assert_eq!(
            authorize_action(&scope, None).unwrap(),
            ApprovalDecision::RequiresApproval
        );
        let exact_grant = explicitly_approve(request_approval(&scope).unwrap().unwrap());
        assert_eq!(
            authorize_action(&scope, Some(exact_grant)).unwrap(),
            ApprovalDecision::Allowed
        );
        let stale_grant = explicitly_approve(request_approval(&scope).unwrap().unwrap());
        let mut changed = scope.clone();
        changed.target.push_str(".new");
        assert_eq!(
            authorize_action(&changed, Some(stale_grant)).unwrap(),
            ApprovalDecision::StaleApproval
        );
    }
}
