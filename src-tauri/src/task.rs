use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::Serialize;

use crate::DeskActError;

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
