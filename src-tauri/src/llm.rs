//! Versioned action-plan schema and local semantic validation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::DeskActError;

const ACTION_PLAN_VERSION: u32 = 1;
const MAX_ACTION_PLAN_BYTES: usize = 32 * 1024;
const MAX_ACTION_PLAN_STEPS: usize = 20;
const MAX_ACTION_TEXT_LENGTH: usize = 500;
const MAX_OPERATION_LENGTH: usize = 80;

/// A complete, locally validated proposal from the primary model; not authorization to execute.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionPlan {
    /// Version of DeskAct's action-plan JSON contract.
    pub version: u32,
    /// Concise human-readable statement of the proposed outcome.
    pub summary: String,
    /// Bounded ordered proposal steps and their expected outcomes.
    pub steps: Vec<ActionPlanStep>,
}

/// One proposed operation and its locally interpreted parameters and postcondition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActionPlanStep {
    /// Operation identifier; execution support is determined later by Rust policy.
    pub operation: String,
    /// Structured operation parameters, never treated as permission by themselves.
    pub parameters: BTreeMap<String, Value>,
    /// Observable outcome expected if the operation succeeds.
    pub expected_outcome: String,
}

/// Parse and semantically validate a complete version-1 action plan.
pub fn validate_action_plan_json(response: &str) -> Result<ActionPlan, DeskActError> {
    if response.len() > MAX_ACTION_PLAN_BYTES {
        return Err(DeskActError::new(format!(
            "Action plan exceeds the {MAX_ACTION_PLAN_BYTES}-byte size limit"
        )));
    }
    let plan: ActionPlan = serde_json::from_str(response).map_err(|error| {
        DeskActError::new(format!(
            "Primary model returned invalid action-plan JSON: {error}"
        ))
    })?;
    if plan.version != ACTION_PLAN_VERSION {
        return Err(DeskActError::new(format!(
            "Unsupported action-plan version {}; expected {ACTION_PLAN_VERSION}",
            plan.version
        )));
    }
    validate_text(&plan.summary, MAX_ACTION_TEXT_LENGTH, "summary")?;
    if plan.steps.is_empty() || plan.steps.len() > MAX_ACTION_PLAN_STEPS {
        return Err(DeskActError::new(format!(
            "Action plan must contain between 1 and {MAX_ACTION_PLAN_STEPS} steps"
        )));
    }
    for (index, step) in plan.steps.iter().enumerate() {
        validate_operation(&step.operation, index + 1)?;
        validate_text(
            &step.expected_outcome,
            MAX_ACTION_TEXT_LENGTH,
            &format!("step {} expected outcome", index + 1),
        )?;
    }
    Ok(plan)
}

fn validate_text(value: &str, max_length: usize, field: &str) -> Result<(), DeskActError> {
    if value.trim().is_empty()
        || value.chars().count() > max_length
        || value.chars().any(char::is_control)
    {
        return Err(DeskActError::new(format!(
            "Action plan {field} must be non-empty, at most {max_length} characters, and contain no control characters"
        )));
    }
    Ok(())
}

fn validate_operation(operation: &str, step: usize) -> Result<(), DeskActError> {
    let mut chars = operation.chars();
    let valid_first = chars
        .next()
        .is_some_and(|character| character.is_ascii_lowercase());
    let valid_rest = chars.all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || matches!(character, '_' | '.' | '-')
    });
    if !valid_first || !valid_rest || operation.len() > MAX_OPERATION_LENGTH {
        return Err(DeskActError::new(format!(
            "Action plan step {step} operation must be a lowercase identifier of at most {MAX_OPERATION_LENGTH} characters"
        )));
    }
    Ok(())
}
