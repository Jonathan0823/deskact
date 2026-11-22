//! Action classification and exact-scope approval policy.

use std::collections::{BTreeMap, VecDeque};

use serde::Serialize;
use serde_json::{Value, json};

use crate::{
    AppCatalogEntry, DeskActError, DiscoveredApp, DiscoverySource, NativeTargetResolution,
    NativeWindow, ResolvedNativeTarget,
};

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

impl ApprovalGrant {
    /// Exact action scope this grant authorizes.
    pub(crate) fn scope(&self) -> &ActionScope {
        &self.scope
    }
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

/// Seam through which an authorized action scope reaches the desktop.
///
/// Callers dispatch only after `authorize_action` allows the exact scope; the adapter performs
/// the operation against the catalog entry named by the scope target and reports native evidence.
pub trait ActionAdapter {
    /// Perform one authorized action scope whose target is an ID in `catalog`.
    fn dispatch(
        &mut self,
        scope: &ActionScope,
        catalog: &[AppCatalogEntry],
    ) -> Result<NativeTargetResolution, DeskActError>;

    /// Re-resolve an app after the user manually restored its minimized window; never relaunches it.
    ///
    /// It only re-observes native state and performs no action, so it is not a separately
    /// authorized dispatch.
    fn resume_after_manual_restore(
        &mut self,
        app: DiscoveredApp,
    ) -> Result<NativeTargetResolution, DeskActError>;
}

/// Adapter that records dispatched scopes and replays scripted outcomes without changing external state.
#[derive(Default)]
pub(crate) struct RecordingActionAdapter {
    /// Scopes dispatched after authorization, in order.
    pub(crate) executed: Vec<ActionScope>,
    /// App names re-resolved after a manual restore, in order.
    pub(crate) resumed: Vec<String>,
    /// Outcomes returned by successive dispatch and resume calls.
    pub(crate) outcomes: VecDeque<Result<NativeTargetResolution, DeskActError>>,
}

impl RecordingActionAdapter {
    /// Return the next scripted outcome; an exhausted script reports that nothing was performed.
    fn next_outcome(&mut self) -> Result<NativeTargetResolution, DeskActError> {
        self.outcomes.pop_front().unwrap_or_else(|| {
            Err(DeskActError::new(
                "The recording adapter performs no native action".into(),
            ))
        })
    }
}

impl ActionAdapter for RecordingActionAdapter {
    fn dispatch(
        &mut self,
        scope: &ActionScope,
        _catalog: &[AppCatalogEntry],
    ) -> Result<NativeTargetResolution, DeskActError> {
        self.executed.push(scope.clone());
        self.next_outcome()
    }

    fn resume_after_manual_restore(
        &mut self,
        app: DiscoveredApp,
    ) -> Result<NativeTargetResolution, DeskActError> {
        self.resumed.push(app.name);
        self.next_outcome()
    }
}

/// Result of re-authorizing one action scope immediately before adapter dispatch.
#[allow(clippy::large_enum_variant)] // Short-lived and consumed immediately by the caller.
pub(crate) enum DispatchOutcome {
    /// The policy allowed the exact scope and the adapter returned this outcome.
    Dispatched(Result<NativeTargetResolution, DeskActError>),
    /// The policy withheld dispatch for this reason.
    Withheld(ApprovalDecision),
}

/// Re-evaluate risk and approval for the exact scope, then dispatch it only when allowed.
pub(crate) fn authorize_and_dispatch(
    adapter: &mut impl ActionAdapter,
    scope: &ActionScope,
    approval: Option<ApprovalGrant>,
    catalog: &[AppCatalogEntry],
) -> Result<DispatchOutcome, DeskActError> {
    Ok(match authorize_action(scope, approval)? {
        ApprovalDecision::Allowed => DispatchOutcome::Dispatched(adapter.dispatch(scope, catalog)),
        decision => DispatchOutcome::Withheld(decision),
    })
}

/// Run a deterministic approval scenario through the same authorize-then-dispatch path the executor uses.
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
    // The fixture adapter reports a successful (non-native) outcome so only real failures read as "failed".
    adapter
        .outcomes
        .push_back(Ok(NativeTargetResolution::Ready(ResolvedNativeTarget {
            app: DiscoveredApp {
                name: "Fixture".into(),
                target: "fixture://documents".into(),
                arguments: String::new(),
                working_directory: String::new(),
                source: DiscoverySource::UserConfigured,
            },
            window: NativeWindow {
                id: "fixture".into(),
                title: "Fixture".into(),
                app_name: "Fixture".into(),
                accessible: true,
                minimized: false,
                is_focused: true,
                visible: true,
                pid: 0,
            },
            snapshot_id: "fixture".into(),
            ref_count: 1,
        })));
    let (authorization, dispatch_failed) =
        match authorize_and_dispatch(&mut adapter, &current_scope, grant, &[])? {
            DispatchOutcome::Dispatched(result) => (ApprovalDecision::Allowed, result.is_err()),
            DispatchOutcome::Withheld(decision) => (decision, false),
        };
    let status = match authorization {
        ApprovalDecision::Allowed if dispatch_failed => "failed",
        ApprovalDecision::Allowed => "executed",
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

    /// An allowed action whose adapter dispatch errs is still recorded as attempted, with the error surfaced.
    #[test]
    fn allowed_dispatch_error_records_attempt_and_surfaces_failure() {
        let scope = fixture_scope();
        let grant = explicitly_approve(request_approval(&scope).unwrap().unwrap());
        let mut adapter = RecordingActionAdapter::default();
        adapter
            .outcomes
            .push_back(Err(DeskActError::new("dispatch failed".into())));
        let outcome = authorize_and_dispatch(&mut adapter, &scope, Some(grant), &[]).unwrap();
        assert!(matches!(outcome, DispatchOutcome::Dispatched(Err(_))));
        assert_eq!(adapter.executed, vec![scope]);
    }
}
