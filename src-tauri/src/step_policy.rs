//! Per-step consequential classification and approval gate for the agent loop (POL-01a, ADR 0002).
//!
//! Rust derives a step's class only from facts it observed (accessible names, the current field
//! value, whether a path exists); a step carries no model-supplied risk claim, so model output
//! cannot lower a classification. Anything that cannot be classified is flagged.

use std::fmt;

/// One proposed agent-loop step, described by facts Rust gathered about it.
///
/// Callers fill the fields from the observation and local checks, never from model claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStep {
    /// Click an element (or press Enter on it) identified by its accessible name.
    Click {
        /// Accessible name of the element.
        element_name: String,
        /// Whether the element sits in a confirmation dialog.
        in_confirmation_dialog: bool,
    },
    /// Press a key chord in the target window.
    Press {
        /// Key sequence such as `Ctrl+S` or `Enter`.
        keys: String,
        /// Accessible name of the focused element, when known; used for Enter.
        focused_element_name: Option<String>,
    },
    /// Replace a field value.
    SetValue {
        /// Current field value read by Rust before the step; `None` if it could not be read.
        current_value: Option<String>,
    },
    /// Confirm a Save As dialog with a path.
    SaveAs {
        /// Whether the path already exists on disk; `None` if it could not be checked.
        path_exists: Option<bool>,
    },
    /// A step Rust could not interpret (unknown tool, malformed arguments, missing facts).
    Unclassifiable {
        /// Why the step could not be classified.
        reason: String,
    },
}

/// Why a step needs approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepRisk {
    /// A POL-01a rule matched.
    Consequential,
    /// No rule could classify the step.
    Unclassifiable,
}

/// Approval requirement for a flagged step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepFlag {
    /// Which kind of flag fired.
    pub risk: StepRisk,
    /// Human-readable rule description shown in the approval prompt.
    pub reason: String,
}

/// Classification of one step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepClass {
    /// No approval needed.
    Routine,
    /// Action-specific approval required before executing.
    NeedsApproval(StepFlag),
}

const RISKY_NAME_WORDS: [&str; 13] = [
    "delete",
    "remove",
    "send",
    "submit",
    "replace",
    "overwrite",
    "buy",
    "pay",
    "purchase",
    "install",
    "uninstall",
    "format",
    "empty",
];
const CONFIRM_NAMES: [&str; 3] = ["yes", "ok", "confirm"];
const RISKY_KEYS: [&[&str]; 4] = [
    &["delete"],
    &["delete", "shift"],
    &["alt", "f4"],
    &["ctrl", "w"],
];

fn consequential(reason: String) -> StepClass {
    StepClass::NeedsApproval(StepFlag {
        risk: StepRisk::Consequential,
        reason,
    })
}

fn unclassifiable(reason: String) -> StepClass {
    StepClass::NeedsApproval(StepFlag {
        risk: StepRisk::Unclassifiable,
        reason,
    })
}

/// Classify a step per POL-01a. Plain saves and ordinary interaction are `Routine`.
pub fn classify_step(step: &AgentStep) -> StepClass {
    match step {
        AgentStep::Click {
            element_name,
            in_confirmation_dialog,
        } => classify_element(element_name, *in_confirmation_dialog),
        AgentStep::Press {
            keys,
            focused_element_name,
        } => classify_press(keys, focused_element_name.as_deref()),
        AgentStep::SetValue { current_value } => match current_value {
            Some(text) if !text.trim().is_empty() => {
                consequential("replaces text that is already in the field".into())
            }
            Some(_) => StepClass::Routine,
            None => unclassifiable("the field's current value could not be read".into()),
        },
        AgentStep::SaveAs { path_exists } => match path_exists {
            Some(true) => consequential("saves over a file that already exists".into()),
            Some(false) => StepClass::Routine,
            None => unclassifiable("could not check whether the save path exists".into()),
        },
        AgentStep::Unclassifiable { reason } => unclassifiable(reason.clone()),
    }
}

fn name_tokens(name: &str) -> Vec<String> {
    name.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

fn classify_element(name: &str, in_confirmation_dialog: bool) -> StepClass {
    let tokens = name_tokens(name);
    let risky_word = tokens
        .iter()
        .find(|token| RISKY_NAME_WORDS.iter().any(|word| token.starts_with(word)));
    if let Some(word) = risky_word {
        return consequential(format!(
            "targets an element named \"{}\" ({word})",
            name.trim()
        ));
    }
    let sign_out = tokens
        .windows(2)
        .any(|pair| pair[0] == "sign" && pair[1].starts_with("out"))
        || tokens.iter().any(|token| token == "signout");
    if sign_out {
        return consequential(format!(
            "targets an element named \"{}\" (sign out)",
            name.trim()
        ));
    }
    if in_confirmation_dialog && tokens.len() == 1 && CONFIRM_NAMES.contains(&tokens[0].as_str()) {
        return consequential(format!("confirms a dialog with \"{}\"", name.trim()));
    }
    StepClass::Routine
}

fn classify_press(keys: &str, focused_element_name: Option<&str>) -> StepClass {
    let mut parts: Vec<String> = keys
        .split('+')
        .map(|part| match part.trim().to_lowercase().as_str() {
            "control" => "ctrl".into(),
            "del" => "delete".into(),
            other => other.to_owned(),
        })
        .collect();
    if parts.iter().any(String::is_empty) {
        return unclassifiable(format!("key sequence `{}` is malformed", keys.trim()));
    }
    parts.sort();
    for risky in RISKY_KEYS {
        let mut risky: Vec<&str> = risky.to_vec();
        risky.sort_unstable();
        if parts.iter().map(String::as_str).eq(risky) {
            return consequential(format!("presses {}", keys.trim()));
        }
    }
    if matches!(parts.as_slice(), [key] if key == "enter" || key == "return")
        && let Some(name) = focused_element_name
        && let StepClass::NeedsApproval(flag) = classify_element(name, false)
    {
        return StepClass::NeedsApproval(StepFlag {
            reason: format!("presses Enter and {}", flag.reason),
            ..flag
        });
    }
    StepClass::Routine
}

/// Source of explicit, action-specific user approval (console in the dev harness, island later).
pub trait StepApprover {
    /// Show the exact step and reason; return `true` only on an explicit approval.
    fn approve(&mut self, step: &AgentStep, reason: &str) -> bool;
}

/// Whether the loop may execute a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepGate {
    /// Execute the step.
    Proceed,
    /// The user denied a flagged step: end the task without telling the model or retrying.
    StoppedByUser,
}

impl fmt::Display for StepGate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Proceed => f.write_str("proceed"),
            Self::StoppedByUser => f.write_str("stopped by user"),
        }
    }
}

/// Classify a step and, when flagged, ask the approver; routine steps never prompt.
pub fn gate_step(step: &AgentStep, approver: &mut impl StepApprover) -> StepGate {
    match classify_step(step) {
        StepClass::Routine => StepGate::Proceed,
        StepClass::NeedsApproval(flag) if approver.approve(step, &flag.reason) => StepGate::Proceed,
        StepClass::NeedsApproval(_) => StepGate::StoppedByUser,
    }
}
