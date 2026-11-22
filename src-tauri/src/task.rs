use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use std::future::Future;

use serde::Serialize;

use crate::{
    ActionAdapter, ActionPlan, AppCatalogEntry, DeskActError, DiscoveredApp, NativeWindow,
    PreparedTask, TaskExecutionReport, execute_prepared_task, model_candidate_context,
    registered_open_workflow, request_action_plan_with_context, validate_open_app_request,
};

/// Port through which the primary model proposes an action plan for an unmatched request.
pub trait ActionPlanner {
    /// Propose an action plan grounded in the model-safe catalog candidate context.
    fn plan(
        &self,
        request: &str,
        candidate_context: &str,
    ) -> impl Future<Output = Result<ActionPlan, DeskActError>> + Send;
}

/// Planner backed by the configured primary model's Chat Completions interface.
pub struct ChatCompletionsPlanner;

impl ActionPlanner for ChatCompletionsPlanner {
    async fn plan(
        &self,
        request: &str,
        candidate_context: &str,
    ) -> Result<ActionPlan, DeskActError> {
        request_action_plan_with_context(request, Some(candidate_context)).await
    }
}

/// Port that asks the user to restore a minimized window manually before resolution resumes.
pub trait ManualRestorePrompt {
    /// Return `Ok(true)` once the user reports the window restored, or `Ok(false)` if they cancel.
    fn confirm_restored(
        &mut self,
        app: &DiscoveredApp,
        window: &NativeWindow,
    ) -> Result<bool, DeskActError>;
}

/// Validate a request, route it through a registered workflow or the primary model, and dispatch it.
///
/// The catalog is loaded only after the request passes its limits. Planning makes no native
/// calls; dispatch goes through `execute_prepared_task` with no approval grants.
pub async fn run_task(
    request: &str,
    load_catalog: impl FnOnce() -> Result<Vec<AppCatalogEntry>, DeskActError>,
    planner: &impl ActionPlanner,
    adapter: &mut impl ActionAdapter,
    restore: &mut dyn ManualRestorePrompt,
    coordinator: &TaskCoordinator,
    cancellation: &CancellationToken,
) -> Result<TaskExecutionReport, DeskActError> {
    let prepared = prepare_task_request(request, load_catalog, planner).await?;
    execute_prepared_task(
        prepared,
        Vec::new(),
        adapter,
        Some(restore),
        coordinator,
        cancellation,
    )
}

/// Validate request limits and scope, then build a plan from a registered workflow or the primary model.
pub async fn prepare_task_request(
    request: &str,
    load_catalog: impl FnOnce() -> Result<Vec<AppCatalogEntry>, DeskActError>,
    planner: &impl ActionPlanner,
) -> Result<PreparedTask, DeskActError> {
    if request.trim().is_empty()
        || request.chars().count() > 4_000
        || request.chars().any(char::is_control)
    {
        return Err(DeskActError::new(
            "Task request must be non-empty, at most 4,000 characters, and contain no control characters".into(),
        ));
    }
    let catalog = load_catalog()?;
    validate_open_app_request(request, &catalog)?;
    let (plan, route) = match registered_open_workflow(request, &catalog)? {
        Some((plan, _entry)) => (plan, "registered_workflow"),
        None => {
            let context = model_candidate_context(&catalog)?;
            let plan = planner.plan(request, &context).await?;
            validate_single_open_app_plan(&plan)?;
            (plan, "primary_model")
        }
    };
    Ok(PreparedTask {
        plan,
        catalog,
        route: route.to_owned(),
    })
}

fn validate_single_open_app_plan(plan: &ActionPlan) -> Result<(), DeskActError> {
    if plan.steps.len() != 1 || plan.steps[0].operation != "open_app" {
        return Err(DeskActError::new(
            "The action plan must contain exactly one supported open_app step; no actions were executed".into(),
        ));
    }
    Ok(())
}

/// Process-local gate that allows only one mutating task to execute at a time.
#[derive(Default)]
pub struct TaskCoordinator {
    mutation_active: AtomicBool,
}

impl TaskCoordinator {
    /// Claim the mutation slot, returning `None` when another task already owns it.
    pub fn try_start_mutating(&self) -> Option<MutationLease<'_>> {
        self.mutation_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| MutationLease { coordinator: self })
    }
}

/// RAII ownership of the process-local mutation slot.
pub struct MutationLease<'a> {
    coordinator: &'a TaskCoordinator,
}

impl Drop for MutationLease<'_> {
    fn drop(&mut self) {
        self.coordinator
            .mutation_active
            .store(false, Ordering::Release);
    }
}

/// Thread-safe cancellation signal shared with an in-flight task.
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// Request cancellation; the task checks this before beginning another step.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// Whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Observable result of a developer-harness task recovery scenario.
#[derive(Debug, Serialize)]
pub struct TaskSimulationReport {
    /// Final task lifecycle status.
    pub status: &'static str,
    /// Number of mutation invocations made by the scenario.
    pub mutation_attempts: usize,
    /// Number of post-state observations made after uncertain delivery.
    pub post_state_observations: usize,
    /// Number of repeated mutation invocations.
    pub retry_count: usize,
    /// Delivery disposition reported by the simulated tool.
    pub delivery_disposition: &'static str,
    /// Ordered events emitted during the simulation.
    pub events: Vec<&'static str>,
    /// Stable reason code when the task cannot complete.
    pub reason: Option<&'static str>,
    /// Actionable explanation suitable for harness output.
    pub message: &'static str,
    /// Mutating requests waiting behind an active task.
    pub queued_requests: usize,
    /// Planned steps not started when execution stopped.
    pub unstarted_steps: usize,
}

/// Whether a native-tool action was applied, refused, or could not be confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryDisposition {
    /// The tool confirmed that the action was not delivered.
    NotDelivered,
    /// The tool may have delivered the action but did not verify its postcondition.
    DeliveredUnverified,
    /// The tool could not establish whether the action was delivered.
    Unknown,
    /// The tool verified its declared postcondition.
    Verified,
}

/// State of the task-level postcondition after an observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostconditionState {
    Satisfied,
    NotSatisfied,
    Unknown,
}

/// Policy decision after evaluating tool disposition and available evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryDecision {
    Continue,
    Retry,
    Stop,
}

/// Decide whether another action attempt is allowed after the postcondition is assessed.
///
/// For uncertain or delivered-but-unverified mutations, callers must observe actual
/// post-state first and pass that evidence here; uncertain evidence never authorizes retry.
pub fn recovery_decision(
    delivery: DeliveryDisposition,
    postcondition: PostconditionState,
    retry_is_safe: bool,
    retries_remaining: usize,
) -> RecoveryDecision {
    match postcondition {
        PostconditionState::Satisfied => RecoveryDecision::Continue,
        PostconditionState::NotSatisfied => match delivery {
            DeliveryDisposition::NotDelivered
            | DeliveryDisposition::Unknown
            | DeliveryDisposition::DeliveredUnverified
                if retry_is_safe && retries_remaining > 0 =>
            {
                RecoveryDecision::Retry
            }
            DeliveryDisposition::NotDelivered
            | DeliveryDisposition::Unknown
            | DeliveryDisposition::DeliveredUnverified
            | DeliveryDisposition::Verified => RecoveryDecision::Stop,
        },
        PostconditionState::Unknown => match delivery {
            DeliveryDisposition::NotDelivered if retry_is_safe && retries_remaining > 0 => {
                RecoveryDecision::Retry
            }
            DeliveryDisposition::NotDelivered
            | DeliveryDisposition::Unknown
            | DeliveryDisposition::DeliveredUnverified
            | DeliveryDisposition::Verified => RecoveryDecision::Stop,
        },
    }
}

/// Whether an observation must be refreshed before using dependent references.
pub fn requires_fresh_observation(stale_reference: bool, observation_complete: bool) -> bool {
    stale_reference || !observation_complete
}

/// Whether a task may resume after UI interference invalidates its previous context.
pub fn may_resume_after_interference(
    observation_complete: bool,
    plan_assumptions_valid: bool,
) -> bool {
    observation_complete && plan_assumptions_valid
}

/// Run a deterministic developer-harness scenario through task recovery policy.
pub fn simulate_task_scenario(scenario: &str) -> Result<TaskSimulationReport, DeskActError> {
    match scenario {
        "unknown-delivery" => Ok(simulate_unknown_delivery(
            false,
            PostconditionState::Unknown,
        )),
        "unknown-delivery-achieved" => Ok(simulate_unknown_delivery(
            false,
            PostconditionState::Satisfied,
        )),
        "unknown-delivery-safe-retry" => Ok(simulate_unknown_delivery(
            true,
            PostconditionState::NotSatisfied,
        )),
        "incomplete-observation" => Ok(simulate_stale_reference_state(false, false)),
        "ui-interference-unclear" => Ok(simulate_ui_interference_state(false, true)),
        "stale-reference" => Ok(simulate_stale_reference()),
        "permission-denied" => Ok(simulate_permission_denied()),
        "busy-request" => Ok(simulate_busy_request()),
        "cancel-in-flight" => Ok(simulate_cancel_in_flight()),
        "ui-interference" => Ok(simulate_ui_interference()),
        _ => Err(DeskActError::new(format!(
            "Unknown task simulation scenario `{scenario}`."
        ))),
    }
}

fn simulate_unknown_delivery(
    retry_is_safe: bool,
    postcondition: PostconditionState,
) -> TaskSimulationReport {
    let mut events = vec!["mutation_started", "tool_returned_unknown"];
    let delivery = DeliveryDisposition::Unknown;

    // The delivery is uncertain, so the runner must inspect post-state before deciding.
    events.push("post_state_observed");
    let decision = recovery_decision(delivery, postcondition, retry_is_safe, 1);
    let retry_count = usize::from(decision == RecoveryDecision::Retry);
    events.push(match decision {
        RecoveryDecision::Continue => "postcondition_verified",
        RecoveryDecision::Retry => "retry_authorized",
        RecoveryDecision::Stop => "retry_suppressed",
    });
    if retry_count > 0 {
        events.extend(["retry_started", "step_verified"]);
    }
    let completed = matches!(
        decision,
        RecoveryDecision::Continue | RecoveryDecision::Retry
    );

    TaskSimulationReport {
        status: if completed { "completed" } else { "partial" },
        mutation_attempts: 1 + retry_count,
        post_state_observations: 1,
        retry_count,
        delivery_disposition: if retry_count > 0 {
            "verified"
        } else {
            "unknown"
        },
        events,
        reason: (!completed).then_some("delivery_uncertain"),
        message: if completed {
            "The postcondition is verified; no duplicate mutation was needed."
        } else {
            "Mutation delivery remains uncertain; inspect fresh post-state before deciding whether to retry."
        },
        queued_requests: 0,
        unstarted_steps: 0,
    }
}

fn simulate_stale_reference() -> TaskSimulationReport {
    simulate_stale_reference_state(true, true)
}

fn simulate_stale_reference_state(
    stale_reference: bool,
    observation_complete: bool,
) -> TaskSimulationReport {
    let needs_refresh = requires_fresh_observation(stale_reference, observation_complete);
    let mut events = vec!["step_started"];
    if stale_reference {
        events.push("stale_reference_rejected");
    }
    if !observation_complete {
        events.push("incomplete_observation_rejected");
    }
    if needs_refresh {
        events.extend(["fresh_observation", "reference_reidentified"]);
    }
    events.extend(["dependent_action_started", "step_verified"]);
    TaskSimulationReport {
        status: "completed",
        mutation_attempts: 1,
        post_state_observations: 1,
        retry_count: 0,
        delivery_disposition: "not_delivered",
        events,
        reason: None,
        message: "A fresh complete observation was used to re-identify the target before continuing.",
        queued_requests: 0,
        unstarted_steps: 0,
    }
}

fn simulate_permission_denied() -> TaskSimulationReport {
    TaskSimulationReport {
        status: "failed",
        mutation_attempts: 1,
        post_state_observations: 0,
        retry_count: 0,
        delivery_disposition: "not_delivered",
        events: vec![
            "step_started",
            "mutation_started",
            "permission_denied",
            "task_failed",
        ],
        reason: Some("permission_denied"),
        message: "Permission denied; grant the required access, then resubmit the request.",
        queued_requests: 0,
        unstarted_steps: 0,
    }
}

fn simulate_busy_request() -> TaskSimulationReport {
    let coordinator = TaskCoordinator::default();
    let _active_task = coordinator
        .try_start_mutating()
        .expect("first task claims the mutation slot");
    let second_request_accepted = coordinator.try_start_mutating().is_some();

    TaskSimulationReport {
        status: if second_request_accepted {
            "running"
        } else {
            "busy"
        },
        mutation_attempts: 1,
        post_state_observations: 0,
        retry_count: 0,
        delivery_disposition: "not_delivered",
        events: if second_request_accepted {
            vec!["mutating_task_started", "second_task_started"]
        } else {
            vec!["mutating_task_started", "request_rejected_busy"]
        },
        reason: (!second_request_accepted).then_some("busy"),
        message: if second_request_accepted {
            "The second mutating task started."
        } else {
            "A mutating task is already active; this request was not queued. Resubmit after it finishes."
        },
        queued_requests: 0,
        unstarted_steps: 0,
    }
}

fn simulate_cancel_in_flight() -> TaskSimulationReport {
    let cancellation = CancellationToken::default();
    cancellation.cancel();
    let next_step_would_start = !cancellation.is_cancelled();

    TaskSimulationReport {
        status: if next_step_would_start {
            "running"
        } else {
            "cancelled"
        },
        mutation_attempts: 1,
        post_state_observations: 0,
        retry_count: 0,
        delivery_disposition: "unknown",
        events: if next_step_would_start {
            vec!["mutation_started", "next_step_started"]
        } else {
            vec![
                "mutation_started",
                "cancellation_received",
                "in_flight_delivery_unknown",
            ]
        },
        reason: (!next_step_would_start).then_some("cancelled_with_in_flight_uncertainty"),
        message: if next_step_would_start {
            "The next task step started."
        } else {
            "Cancellation stopped further steps; in-flight delivery is unknown. Inspect fresh post-state before retrying."
        },
        queued_requests: 0,
        unstarted_steps: 1,
    }
}

fn simulate_ui_interference() -> TaskSimulationReport {
    simulate_ui_interference_state(true, true)
}

fn simulate_ui_interference_state(
    observation_complete: bool,
    plan_assumptions_valid: bool,
) -> TaskSimulationReport {
    let may_resume = may_resume_after_interference(observation_complete, plan_assumptions_valid);
    TaskSimulationReport {
        status: if may_resume { "completed" } else { "partial" },
        mutation_attempts: 1,
        post_state_observations: 1,
        retry_count: 0,
        delivery_disposition: "verified",
        events: if may_resume {
            vec![
                "task_paused",
                "fresh_observation",
                "plan_revalidated",
                "task_resumed",
                "step_verified",
            ]
        } else {
            vec!["task_paused", "user_input_required"]
        },
        reason: (!may_resume).then_some("safe_resumption_unclear"),
        message: if may_resume {
            "A fresh complete observation and valid plan assumptions allowed safe resumption."
        } else {
            "The task is paused because safe resumption is unclear; user input is required."
        },
        queued_requests: 0,
        unstarted_steps: 0,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, sync::Mutex};

    use serde_json::json;

    use super::*;
    use crate::{
        ActionPlanStep, ActionScope, AppCatalogSource, ApprovalGrant, NativeTargetResolution,
        ResolvedNativeTarget, approval::RecordingActionAdapter, explicitly_approve, open_app_plan,
        request_approval,
    };

    /// Planner fixture that returns one scripted plan and records the candidate context it received.
    struct FixturePlanner {
        plan: Option<ActionPlan>,
        contexts: Mutex<Vec<String>>,
    }

    impl ActionPlanner for FixturePlanner {
        async fn plan(
            &self,
            _request: &str,
            candidate_context: &str,
        ) -> Result<ActionPlan, DeskActError> {
            self.contexts
                .lock()
                .unwrap()
                .push(candidate_context.to_owned());
            self.plan
                .clone()
                .ok_or_else(|| DeskActError::new("fixture planner was not expected".into()))
        }
    }

    /// Restore prompt fixture that replays scripted user answers.
    struct ScriptedRestore {
        answers: Vec<bool>,
        prompts: Vec<String>,
    }

    impl ManualRestorePrompt for ScriptedRestore {
        fn confirm_restored(
            &mut self,
            _app: &DiscoveredApp,
            window: &NativeWindow,
        ) -> Result<bool, DeskActError> {
            self.prompts.push(window.title.clone());
            Ok(!self.answers.is_empty() && self.answers.remove(0))
        }
    }

    fn calculator() -> AppCatalogEntry {
        AppCatalogEntry {
            id: "calculator-id".into(),
            name: "Calculator".into(),
            target: "C:\\Windows\\System32\\calc.exe".into(),
            arguments: String::new(),
            working_directory: String::new(),
            source: AppCatalogSource::UserConfigured,
            aliases: Vec::new(),
        }
    }

    fn window(minimized: bool) -> NativeWindow {
        NativeWindow {
            id: "window-1".into(),
            title: "Calculator".into(),
            app_name: "Calculator".into(),
            accessible: true,
            minimized,
            is_focused: false,
            visible: true,
            pid: 1,
        }
    }

    fn ready() -> Result<NativeTargetResolution, DeskActError> {
        Ok(NativeTargetResolution::Ready(ResolvedNativeTarget {
            app: calculator().as_discovered_app(),
            window: window(false),
            snapshot_id: "snapshot-1".into(),
            ref_count: 3,
        }))
    }

    fn minimized() -> Result<NativeTargetResolution, DeskActError> {
        Ok(NativeTargetResolution::NeedsManualRestore {
            app: calculator().as_discovered_app(),
            window: window(true),
        })
    }

    fn open_scope() -> ActionScope {
        ActionScope {
            action: "open_app".into(),
            target: "calculator-id".into(),
            parameters: BTreeMap::from([("app_id".into(), json!("calculator-id"))]),
        }
    }

    /// Run `run_task` on a current-thread runtime with one Calculator catalog entry.
    fn run(
        request: &str,
        planner: &FixturePlanner,
        adapter: &mut RecordingActionAdapter,
        restore: &mut ScriptedRestore,
    ) -> Result<TaskExecutionReport, DeskActError> {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(run_task(
                request,
                || Ok(vec![calculator()]),
                planner,
                adapter,
                restore,
                &TaskCoordinator::default(),
                &CancellationToken::default(),
            ))
    }

    fn no_planner() -> FixturePlanner {
        FixturePlanner {
            plan: None,
            contexts: Mutex::default(),
        }
    }

    fn no_restore() -> ScriptedRestore {
        ScriptedRestore {
            answers: Vec::new(),
            prompts: Vec::new(),
        }
    }

    /// Dispatch a Calculator plan whose second step uses `operation`.
    fn execute_with_second_step(
        operation: &str,
        approvals: Vec<ApprovalGrant>,
        adapter: &mut RecordingActionAdapter,
    ) -> Result<TaskExecutionReport, DeskActError> {
        let mut plan = open_app_plan(&calculator());
        plan.steps.push(ActionPlanStep {
            operation: operation.into(),
            ..plan.steps[0].clone()
        });
        execute_prepared_task(
            PreparedTask {
                plan,
                catalog: vec![calculator()],
                route: "primary_model".into(),
            },
            approvals,
            adapter,
            None,
            &TaskCoordinator::default(),
            &CancellationToken::default(),
        )
    }

    /// An exact catalog name takes the registered workflow and never consults the planner.
    #[test]
    fn registered_route_dispatches_the_exact_scope_without_planning() {
        let planner = no_planner();
        let mut adapter = RecordingActionAdapter::default();
        adapter.outcomes.push_back(ready());

        let report = run("open Calculator", &planner, &mut adapter, &mut no_restore()).unwrap();

        assert_eq!(report.route, "registered_workflow");
        assert_eq!(report.status, "completed");
        assert_eq!(report.evidence[0].snapshot_id, "snapshot-1");
        assert_eq!(adapter.executed, vec![open_scope()]);
        assert!(planner.contexts.lock().unwrap().is_empty());
    }

    /// An unmatched request is planned by the primary model from IDs and display names only.
    #[test]
    fn model_route_plans_from_catalog_candidates_then_dispatches() {
        let planner = FixturePlanner {
            plan: Some(open_app_plan(&calculator())),
            contexts: Mutex::default(),
        };
        let mut adapter = RecordingActionAdapter::default();
        adapter.outcomes.push_back(ready());

        let report = run(
            "open my calculator",
            &planner,
            &mut adapter,
            &mut no_restore(),
        )
        .unwrap();

        assert_eq!(report.route, "primary_model");
        assert_eq!(report.status, "completed");
        assert_eq!(adapter.executed, vec![open_scope()]);
        let contexts = planner.contexts.lock().unwrap();
        assert!(contexts[0].contains("calculator-id"));
        assert!(!contexts[0].contains("calc.exe"));
    }

    /// An operation without a local risk classification rejects the plan before any dispatch.
    #[test]
    fn unclassified_operation_is_rejected_before_dispatch() {
        let mut adapter = RecordingActionAdapter::default();
        let error = execute_with_second_step("click", Vec::new(), &mut adapter).unwrap_err();
        assert!(error.to_string().contains("no local risk classification"));
        assert!(error.to_string().contains("no actions were executed"));
        assert!(adapter.executed.is_empty());
    }

    /// A consequential step needs an exact grant, and even then this executor does not support it.
    #[test]
    fn consequential_operation_without_grant_is_rejected_before_dispatch() {
        let mut adapter = RecordingActionAdapter::default();
        let error = execute_with_second_step("delete_file", Vec::new(), &mut adapter).unwrap_err();
        assert!(error.to_string().contains("without an explicit approval"));
        assert!(adapter.executed.is_empty());

        let scope = ActionScope {
            action: "delete_file".into(),
            ..open_scope()
        };
        let grant = explicitly_approve(request_approval(&scope).unwrap().unwrap());
        let error = execute_with_second_step("delete_file", vec![grant], &mut adapter).unwrap_err();
        assert!(error.to_string().contains("unsupported operation"));
        assert!(adapter.executed.is_empty());
    }

    /// A second mutating task is refused while the mutation lease is held.
    #[test]
    fn busy_mutation_lease_rejects_the_task_before_dispatch() {
        let coordinator = TaskCoordinator::default();
        let _active = coordinator.try_start_mutating().unwrap();
        let mut adapter = RecordingActionAdapter::default();

        let error = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(run_task(
                "open Calculator",
                || Ok(vec![calculator()]),
                &no_planner(),
                &mut adapter,
                &mut no_restore(),
                &coordinator,
                &CancellationToken::default(),
            ))
            .unwrap_err();

        assert!(error.to_string().contains("already active"));
        assert!(adapter.executed.is_empty());
    }

    /// Cancellation before a step leaves it and every later step not attempted.
    #[test]
    fn cancellation_reports_remaining_steps_as_not_attempted() {
        let cancellation = CancellationToken::default();
        cancellation.cancel();
        let mut adapter = RecordingActionAdapter::default();

        let report = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(run_task(
                "open Calculator",
                || Ok(vec![calculator()]),
                &no_planner(),
                &mut adapter,
                &mut no_restore(),
                &TaskCoordinator::default(),
                &cancellation,
            ))
            .unwrap();

        assert_eq!(report.status, "failed");
        assert_eq!(
            report.unresolved_steps,
            vec!["Step 1 (open Calculator): not attempted because the task was cancelled"]
        );
        assert!(adapter.executed.is_empty());
    }

    /// A minimized window is re-resolved after the user confirms a manual restore.
    #[test]
    fn minimized_window_resumes_after_manual_restore() {
        let mut adapter = RecordingActionAdapter::default();
        adapter.outcomes.extend([minimized(), ready()]);
        let mut restore = ScriptedRestore {
            answers: vec![true],
            prompts: Vec::new(),
        };

        let report = run("open Calculator", &no_planner(), &mut adapter, &mut restore).unwrap();

        assert_eq!(report.status, "completed");
        assert_eq!(restore.prompts, vec!["Calculator"]);
        assert_eq!(adapter.executed.len(), 1);
        assert_eq!(adapter.resumed, vec!["Calculator"]);
    }

    /// Declining the restore prompt stops the step without re-resolving or relaunching.
    #[test]
    fn cancelled_manual_restore_is_unresolved() {
        let mut adapter = RecordingActionAdapter::default();
        adapter.outcomes.push_back(minimized());
        let mut restore = no_restore();

        let report = run("open Calculator", &no_planner(), &mut adapter, &mut restore).unwrap();

        assert_eq!(report.status, "failed");
        assert_eq!(
            report.unresolved_steps,
            vec!["Step 1 (open Calculator): Resolution cancelled; no window was selected"]
        );
        assert_eq!(restore.prompts, vec!["Calculator"]);
        assert!(adapter.resumed.is_empty());
    }

    #[test]
    fn model_plan_must_contain_exactly_one_open_app_step() {
        let mut plan = open_app_plan(&calculator());
        plan.steps.push(plan.steps[0].clone());
        assert!(validate_single_open_app_plan(&plan).is_err());
    }

    #[test]
    fn model_plan_rejects_other_single_step_operations() {
        let mut plan = open_app_plan(&calculator());
        plan.steps[0].operation = "delete_file".into();
        assert!(validate_single_open_app_plan(&plan).is_err());
    }
}
