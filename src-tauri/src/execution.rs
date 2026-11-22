//! Policy-bounded execution of validated model action plans.

use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

use crate::{
    ActionAdapter, ActionPlan, ActionPlanStep, ActionRiskClass, ActionScope, AppCatalogEntry,
    ApprovalGrant, CancellationToken, DeliveryDisposition, DeskActError, DiscoveredApp,
    ManualRestorePrompt, NativeTargetResolution, PostconditionState, RecoveryDecision,
    TaskCoordinator, VerifiedNativeCli,
    approval::{DispatchOutcome, authorize_and_dispatch},
    classify_action, prepare_app_selection, recovery_decision, validate_action_plan_json,
};

/// Public result from one completed task-execution request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskExecutionReport {
    /// Deterministic workflow or primary-model route.
    pub route: String,
    /// Whether the task reached its locally verified postcondition.
    pub status: String,
    /// Validated plan summary.
    pub summary: String,
    /// Postconditions supported by fresh native evidence.
    pub achieved_postconditions: Vec<String>,
    /// Steps whose outcomes could not be verified or that were not attempted.
    pub unresolved_steps: Vec<String>,
    /// Evidence for each native resolution.
    pub evidence: Vec<TaskExecutionEvidence>,
}

/// Native evidence recorded for one supported app-opening step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TaskExecutionEvidence {
    /// Locally selected catalog ID.
    pub app_id: String,
    /// Canonical catalog name.
    pub app_name: String,
    /// Fresh native window identity.
    pub window_id: String,
    /// Fresh complete accessibility snapshot identity.
    pub snapshot_id: String,
    /// Number of actionable references in the snapshot.
    pub ref_count: usize,
}

/// A locally validated app-opening request ready for policy-bounded dispatch.
#[derive(Debug, Clone)]
pub struct PreparedTask {
    pub(crate) plan: ActionPlan,
    pub(crate) catalog: Vec<AppCatalogEntry>,
    pub(crate) route: String,
}

/// Action adapter that launches catalog entries through the verified native CLI.
///
/// The CLI connection is made on first dispatch, so a plan rejected by policy never runs it.
#[derive(Default)]
pub struct NativeActionAdapter {
    cli: Option<VerifiedNativeCli>,
}

impl NativeActionAdapter {
    /// Return the verified CLI, connecting on first use.
    fn cli(&mut self) -> Result<&VerifiedNativeCli, DeskActError> {
        if self.cli.is_none() {
            self.cli = Some(VerifiedNativeCli::connect()?);
        }
        Ok(self.cli.as_ref().expect("native CLI initialized"))
    }
}

impl ActionAdapter for NativeActionAdapter {
    fn dispatch(
        &mut self,
        scope: &ActionScope,
        catalog: &[AppCatalogEntry],
    ) -> Result<NativeTargetResolution, DeskActError> {
        if scope.action != "open_app" {
            return Err(DeskActError::new(format!(
                "The native action adapter cannot perform `{}`; no app was launched",
                scope.action
            )));
        }
        let app = catalog
            .iter()
            .find(|entry| entry.id == scope.target)
            .ok_or_else(|| {
                DeskActError::new(
                    "The action target is outside the local catalog; no app was launched".into(),
                )
            })?;
        self.cli()?.resolve_target(app.as_discovered_app())
    }

    fn resume_after_manual_restore(
        &mut self,
        app: DiscoveredApp,
    ) -> Result<NativeTargetResolution, DeskActError> {
        self.cli()?.resume_after_manual_restore(app)
    }
}

/// Validate that a request is one complete, explicit app-opening intent within the current executor scope.
pub fn validate_open_app_request(
    request: &str,
    catalog: &[AppCatalogEntry],
) -> Result<(), DeskActError> {
    let Some((verb, app_request)) = request.trim().split_once(' ') else {
        return Err(open_scope_error());
    };
    let app_request = app_request.trim();
    if !verb.eq_ignore_ascii_case("open") || app_request.is_empty() {
        return Err(open_scope_error());
    }
    if matches!(
        prepare_app_selection(catalog, app_request),
        Ok(crate::AppSelectionInput::ExactMatch { .. })
    ) {
        return Ok(());
    }
    let normalized = format!(" {} ", app_request.to_lowercase());
    let compound_markers = [
        " and ",
        " or ",
        " then ",
        " after ",
        " before ",
        " to type ",
        " to enter ",
        " to save ",
        " to click ",
        " to press ",
        " to write ",
        " to delete ",
        " to move ",
        " to copy ",
        " to paste ",
        ";",
        ",",
        " & ",
    ];
    if compound_markers
        .iter()
        .any(|marker| normalized.contains(marker))
    {
        return Err(open_scope_error());
    }
    let secondary_actions = [
        "type", "enter", "save", "click", "press", "write", "delete", "move", "copy", "paste",
        "select", "play", "open", "launch",
    ];
    if app_request
        .split_whitespace()
        .any(|word| secondary_actions.contains(&word.to_lowercase().as_str()))
    {
        return Err(open_scope_error());
    }
    Ok(())
}

/// Return the shared actionable error for requests outside the open-app-only scope.
fn open_scope_error() -> DeskActError {
    DeskActError::new(
        "This executor supports only one explicit `open <app>` intent; compound or other requests are unsupported".into(),
    )
}

/// Build a deterministic, registered `open <canonical app name>` workflow when uniquely matched.
pub fn registered_open_workflow(
    request: &str,
    catalog: &[AppCatalogEntry],
) -> Result<Option<(ActionPlan, AppCatalogEntry)>, DeskActError> {
    if catalog.is_empty() {
        return Ok(None);
    }
    let Some((verb, app_request)) = request.trim().split_once(' ') else {
        return Ok(None);
    };
    if !verb.eq_ignore_ascii_case("open") {
        return Ok(None);
    }
    let app_request = app_request.trim();
    if app_request.is_empty() {
        return Ok(None);
    }
    match prepare_app_selection(catalog, app_request)? {
        crate::AppSelectionInput::ExactMatch { entry } => {
            let plan = open_app_plan(&entry);
            Ok(Some((plan, entry)))
        }
        crate::AppSelectionInput::NeedsClarification { .. } => Err(DeskActError::new(
            "More than one app exactly matches this request; ask the user to choose. No app was launched.".into(),
        )),
        crate::AppSelectionInput::NeedsModel { .. } => Ok(None),
    }
}

/// Create the local plan used by the deterministic open-app workflow.
pub fn open_app_plan(entry: &AppCatalogEntry) -> ActionPlan {
    let mut parameters = BTreeMap::new();
    parameters.insert("app_id".to_owned(), json!(entry.id));
    ActionPlan {
        version: 1,
        summary: format!("Open {}", entry.name),
        steps: vec![ActionPlanStep {
            operation: "open_app".into(),
            parameters,
            expected_outcome: format!("{} is open", entry.name),
        }],
    }
}

/// Prevalidate the whole plan, then dispatch each authorized step through the action adapter.
///
/// Every step becomes an action scope classified by `classify_action`. An unclassified step, a
/// consequential step without an exact matching approval grant, an operation this executor does
/// not support, shortcut arguments, or an unsupported postcondition rejects the plan before any
/// dispatch. Dispatch holds the mutation lease, checks cancellation before each step, and stops at
/// the first outcome that recovery policy does not let it continue past.
pub fn execute_prepared_task(
    prepared: PreparedTask,
    mut approvals: Vec<ApprovalGrant>,
    adapter: &mut impl ActionAdapter,
    mut restore: Option<&mut dyn ManualRestorePrompt>,
    coordinator: &TaskCoordinator,
    cancellation: &CancellationToken,
) -> Result<TaskExecutionReport, DeskActError> {
    let PreparedTask {
        plan,
        catalog,
        route,
    } = prepared;
    let serialized_plan = serde_json::to_string(&plan)
        .map_err(|_| DeskActError::new("Could not revalidate the action plan".into()))?;
    let plan = validate_action_plan_json(&serialized_plan)?;
    let mut selected = Vec::with_capacity(plan.steps.len());
    for (index, step) in plan.steps.iter().enumerate() {
        if step.parameters.len() != 1 || !step.parameters.contains_key("app_id") {
            return Err(DeskActError::new(format!(
                "Action plan step {} must contain exactly one `app_id` parameter; no actions were executed",
                index + 1
            )));
        }
        let app_id = step.parameters["app_id"].as_str().ok_or_else(|| {
            DeskActError::new(format!(
                "Action plan step {} `app_id` must be a string; no actions were executed",
                index + 1
            ))
        })?;
        let app = catalog
            .iter()
            .find(|entry| entry.id == app_id)
            .cloned()
            .ok_or_else(|| {
                DeskActError::new(format!(
                    "Action plan step {} selected an app ID outside the local catalog; no actions were executed",
                    index + 1
                ))
            })?;
        let scope = ActionScope {
            action: step.operation.clone(),
            target: app.id.clone(),
            parameters: step.parameters.clone(),
        };
        let risk_class = classify_action(&scope).map_err(|error| {
            DeskActError::new(format!(
                "Action plan step {} was denied: {error}; no actions were executed",
                index + 1
            ))
        })?;
        let approval = approvals
            .iter()
            .position(|grant| grant.scope() == &scope)
            .map(|position| approvals.swap_remove(position));
        if risk_class == ActionRiskClass::Consequential && approval.is_none() {
            return Err(DeskActError::new(format!(
                "Action plan step {} `{}` is a consequential action without an explicit approval for its exact scope; no actions were executed",
                index + 1,
                step.operation
            )));
        }
        if scope.action != "open_app" {
            return Err(DeskActError::new(format!(
                "Action plan step {} uses unsupported operation `{}`; no actions were executed",
                index + 1,
                step.operation
            )));
        }
        if !app.arguments.trim().is_empty() {
            return Err(DeskActError::new(format!(
                "Action plan step {} selects `{}` whose shortcut requires unsupported arguments; no actions were executed",
                index + 1,
                app.name
            )));
        }
        let verified_expectation = format!("{} is open", app.name);
        if !step
            .expected_outcome
            .eq_ignore_ascii_case(&verified_expectation)
        {
            return Err(DeskActError::new(format!(
                "Action plan step {} requested an unsupported postcondition; no actions were executed",
                index + 1
            )));
        }
        selected.push((app, scope, approval));
    }

    let Some(_lease) = coordinator.try_start_mutating() else {
        return Err(DeskActError::new(
            "A mutating task is already active; this request was not queued. Resubmit after it finishes. No actions were executed".into(),
        ));
    };
    let mut evidence = Vec::with_capacity(selected.len());
    let mut unresolved_steps = Vec::new();
    let mut cancelled = false;
    for (index, (app, scope, approval)) in selected.iter_mut().enumerate() {
        if cancellation.is_cancelled() {
            cancelled = true;
            break;
        }
        // Only the first dispatch is authorized; manual-restore resumes below merely re-observe.
        let mut outcome = match authorize_and_dispatch(adapter, scope, approval.take(), &catalog) {
            Ok(DispatchOutcome::Dispatched(outcome)) => outcome,
            Ok(DispatchOutcome::Withheld(decision)) => Err(DeskActError::new(format!(
                "the action was not authorized immediately before dispatch ({decision:?})"
            ))),
            Err(error) => Err(error),
        };
        while let Ok(NativeTargetResolution::NeedsManualRestore { app, window }) = &outcome {
            let Some(prompt) = restore.as_deref_mut() else {
                break;
            };
            if cancellation.is_cancelled() {
                cancelled = true;
                outcome = Err(DeskActError::new(
                    "Cancelled while waiting for manual window restoration".into(),
                ));
                break;
            }
            outcome = match prompt.confirm_restored(app, window) {
                Ok(true) if cancellation.is_cancelled() => {
                    cancelled = true;
                    Err(DeskActError::new(
                        "Cancelled while waiting for manual window restoration".into(),
                    ))
                }
                Ok(true) => adapter.resume_after_manual_restore(app.clone()),
                Ok(false) => Err(DeskActError::new(
                    "Resolution cancelled; no window was selected".into(),
                )),
                Err(error) => Err(error),
            };
        }
        let (delivery, postcondition, failure) = match &outcome {
            Ok(NativeTargetResolution::Ready(target))
                if target.window.accessible && !target.window.minimized && target.ref_count > 0 =>
            {
                (
                    DeliveryDisposition::Verified,
                    PostconditionState::Satisfied,
                    String::new(),
                )
            }
            Ok(NativeTargetResolution::Ready(target)) => (
                DeliveryDisposition::DeliveredUnverified,
                PostconditionState::NotSatisfied,
                format!(
                    "resolved window `{}` did not satisfy the accessible, non-minimized snapshot precondition",
                    target.window.id
                ),
            ),
            Ok(NativeTargetResolution::NeedsManualRestore { window, .. }) => (
                DeliveryDisposition::DeliveredUnverified,
                PostconditionState::Unknown,
                format!(
                    "window `{}` is minimized and needs manual restoration",
                    window.title
                ),
            ),
            Err(error) => (
                DeliveryDisposition::Unknown,
                PostconditionState::Unknown,
                error.to_string(),
            ),
        };
        // Relaunching an app is not idempotent, so an unverified open_app is never retried.
        match (
            recovery_decision(delivery, postcondition, false, 0),
            outcome,
        ) {
            (RecoveryDecision::Continue, Ok(NativeTargetResolution::Ready(target))) => {
                evidence.push(TaskExecutionEvidence {
                    app_id: app.id.clone(),
                    app_name: app.name.clone(),
                    window_id: target.window.id,
                    snapshot_id: target.snapshot_id,
                    ref_count: target.ref_count,
                });
            }
            _ => {
                unresolved_steps.push(format!("Step {} (open {}): {failure}", index + 1, app.name));
                break;
            }
        }
    }
    for (index, (app, _, _)) in selected
        .iter()
        .enumerate()
        .skip(evidence.len() + unresolved_steps.len())
    {
        unresolved_steps.push(format!(
            "Step {} (open {}): not attempted {}",
            index + 1,
            app.name,
            if cancelled {
                "because the task was cancelled"
            } else {
                "after an earlier step could not be verified"
            }
        ));
    }

    let achieved_postconditions = evidence
        .iter()
        .map(|evidence| {
            format!(
                "{} is open in one uniquely matched accessible window with a fresh complete snapshot",
                evidence.app_name
            )
        })
        .collect();
    let summary = if evidence.is_empty() {
        "No app window was verified".to_owned()
    } else {
        format!(
            "Opened {}",
            evidence
                .iter()
                .map(|evidence| evidence.app_name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Ok(TaskExecutionReport {
        route,
        status: if unresolved_steps.is_empty() {
            "completed".into()
        } else if evidence.is_empty() {
            "failed".into()
        } else {
            "partial".into()
        },
        summary,
        achieved_postconditions,
        unresolved_steps,
        evidence,
    })
}

/// Prepare the catalog-only model context; executable paths and launch arguments stay local.
pub fn model_candidate_context(catalog: &[AppCatalogEntry]) -> Result<String, DeskActError> {
    let candidates: Vec<_> = catalog
        .iter()
        .map(|entry| json!({ "id": entry.id, "display_name": entry.name }))
        .collect();
    serde_json::to_string(&Value::Array(candidates))
        .map_err(|_| DeskActError::new("Could not serialize app candidates for the model".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AppCatalogSource, NativeWindow, approval::RecordingActionAdapter};

    struct CancellingPrompt(CancellationToken);

    impl ManualRestorePrompt for CancellingPrompt {
        fn confirm_restored(
            &mut self,
            _app: &DiscoveredApp,
            _window: &NativeWindow,
        ) -> Result<bool, DeskActError> {
            self.0.cancel();
            Ok(true)
        }
    }

    /// Cancelling during manual restoration must not resume, and later steps are not attempted.
    #[test]
    fn cancellation_during_manual_restore_stops_without_resuming() {
        let entry = |id: &str, name: &str| AppCatalogEntry {
            id: id.into(),
            name: name.into(),
            target: format!("C:\\Apps\\{name}.exe"),
            arguments: String::new(),
            working_directory: String::new(),
            source: AppCatalogSource::UserConfigured,
            aliases: Vec::new(),
        };
        let (first, second) = (
            entry("first-id", "First App"),
            entry("second-id", "Second App"),
        );
        let plan = ActionPlan {
            version: 1,
            summary: "Open both apps".into(),
            steps: vec![
                open_app_plan(&first).steps[0].clone(),
                open_app_plan(&second).steps[0].clone(),
            ],
        };
        let mut adapter = RecordingActionAdapter::default();
        adapter
            .outcomes
            .push_back(Ok(NativeTargetResolution::NeedsManualRestore {
                app: first.as_discovered_app(),
                window: NativeWindow {
                    id: "w1".into(),
                    title: "First".into(),
                    app_name: "First App".into(),
                    accessible: true,
                    minimized: true,
                    is_focused: false,
                    visible: false,
                    pid: 1,
                },
            }));
        let cancellation = CancellationToken::default();
        let mut prompt = CancellingPrompt(cancellation.clone());

        let report = execute_prepared_task(
            PreparedTask {
                plan,
                catalog: vec![first, second],
                route: "registered_workflow".into(),
            },
            Vec::new(),
            &mut adapter,
            Some(&mut prompt),
            &TaskCoordinator::default(),
            &cancellation,
        )
        .expect("cancelled execution should produce a report");

        assert!(adapter.resumed.is_empty());
        assert_eq!(adapter.executed.len(), 1);
        assert_eq!(report.unresolved_steps.len(), 2);
        assert!(
            report.unresolved_steps[1].contains("not attempted because the task was cancelled")
        );
    }

    /// A native-resolution error must be returned as evidence-backed unresolved work.
    #[test]
    fn native_failure_is_reported_as_an_unresolved_step() {
        let entry = AppCatalogEntry {
            id: "calculator-id".into(),
            name: "Calculator".into(),
            target: "C:\\Windows\\System32\\calc.exe".into(),
            arguments: String::new(),
            working_directory: String::new(),
            source: AppCatalogSource::UserConfigured,
            aliases: Vec::new(),
        };
        let mut adapter = RecordingActionAdapter::default();
        adapter
            .outcomes
            .push_back(Err(DeskActError::new("fixture resolution failure".into())));

        let report = execute_prepared_task(
            PreparedTask {
                plan: open_app_plan(&entry),
                catalog: vec![entry],
                route: "primary_model".into(),
            },
            Vec::new(),
            &mut adapter,
            None,
            &TaskCoordinator::default(),
            &CancellationToken::default(),
        )
        .expect("unverified execution should produce a report");

        assert_eq!(report.status, "failed");
        assert!(report.achieved_postconditions.is_empty());
        assert_eq!(report.unresolved_steps.len(), 1);
        assert!(report.unresolved_steps[0].contains("fixture resolution failure"));
    }

    /// Reject shortcut arguments during whole-plan validation, before any adapter dispatch.
    #[test]
    fn shortcut_arguments_are_rejected_before_resolution() {
        let first = AppCatalogEntry {
            id: "first-id".into(),
            name: "First App".into(),
            target: "C:\\Windows\\First.exe".into(),
            arguments: String::new(),
            working_directory: String::new(),
            source: AppCatalogSource::UserConfigured,
            aliases: Vec::new(),
        };
        let mut second = first.clone();
        second.id = "second-id".into();
        second.name = "Second App".into();
        second.target = "C:\\Windows\\Second.exe".into();
        second.arguments = "--unsupported".into();
        let plan = ActionPlan {
            version: 1,
            summary: "Open both apps".into(),
            steps: vec![
                open_app_plan(&first).steps[0].clone(),
                open_app_plan(&second).steps[0].clone(),
            ],
        };
        let mut adapter = RecordingActionAdapter::default();
        let result = execute_prepared_task(
            PreparedTask {
                plan,
                catalog: vec![first, second],
                route: "registered_workflow".into(),
            },
            Vec::new(),
            &mut adapter,
            None,
            &TaskCoordinator::default(),
            &CancellationToken::default(),
        );
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unsupported arguments")
        );
        assert!(adapter.executed.is_empty());
    }

    /// Reject compound requests instead of running only their first open-app portion.
    #[test]
    fn compound_open_requests_are_outside_the_executor_scope() {
        let entry = AppCatalogEntry {
            id: "calculator-id".into(),
            name: "Calculator".into(),
            target: "C:\\Windows\\System32\\calc.exe".into(),
            arguments: String::new(),
            working_directory: String::new(),
            source: AppCatalogSource::UserConfigured,
            aliases: Vec::new(),
        };
        let error = validate_open_app_request("open Calculator and type 123", &[entry])
            .expect_err("compound intent must be rejected");
        assert!(error.to_string().contains("compound"));
    }
}
