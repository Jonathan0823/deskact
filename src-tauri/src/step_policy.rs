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
    /// Click an element identified by its accessible name and role.
    Click {
        /// Accessible name of the element.
        element_name: String,
        /// Accessible role of the element, shown in the approval prompt.
        element_role: String,
        /// Whether the element sits in a confirmation dialog (any dialog except the Save As file
        /// dialog itself; the "Confirm Save As" overwrite prompt is a confirmation dialog).
        in_confirmation_dialog: bool,
    },
    /// Press a key chord in the target window.
    Press {
        /// Key sequence such as `Ctrl+S` or `Enter`.
        keys: String,
        /// Accessible name of the focused element; `None` when no single focused element is
        /// known (nothing focused, or several nodes claim focus). Used for Enter and Space.
        focused_element_name: Option<String>,
        /// Whether the focused element sits in a confirmation dialog.
        focused_in_confirmation_dialog: bool,
    },
    /// Replace a field value.
    SetValue {
        /// Accessible name of the field.
        element_name: String,
        /// Accessible role of the field.
        element_role: String,
        /// The text that will replace the value.
        new_text: String,
        /// Current field value read by Rust before the step; `None` if it could not be read.
        current_value: Option<String>,
    },
    /// Confirm a Save As dialog with a path.
    SaveAs {
        /// Full path that will be saved, when it could be resolved (absolute, or a bare name
        /// resolved against the dialog's observable current folder).
        path: Option<String>,
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
/// A chord is risky when it contains every key of one of these sets, whatever else is held.
const RISKY_KEYS: [&[&str]; 3] = [&["delete"], &["alt", "f4"], &["ctrl", "w"]];
/// Keys that activate whatever element has focus.
const ACTIVATING_KEYS: [&str; 3] = ["enter", "return", "numpadenter"];

/// Escape on-screen or model text for prompts and reasons: control characters and quotes cannot
/// forge extra lines, terminal escapes or an early end of a quoted name.
pub(crate) fn escape_text(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let escaped: Vec<char> = if c.is_control() || c == '"' || c == '\\' {
                c.escape_default().collect()
            } else {
                vec![c]
            };
            escaped
        })
        .collect()
}

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
            ..
        } => classify_element(element_name, *in_confirmation_dialog),
        AgentStep::Press {
            keys,
            focused_element_name,
            focused_in_confirmation_dialog,
        } => classify_press(
            keys,
            focused_element_name.as_deref(),
            *focused_in_confirmation_dialog,
        ),
        AgentStep::SetValue { current_value, .. } => match current_value {
            Some(text) if !text.trim().is_empty() => {
                consequential("replaces text that is already in the field".into())
            }
            Some(_) => StepClass::Routine,
            None => unclassifiable("the field's current value could not be read".into()),
        },
        AgentStep::SaveAs { path_exists, .. } => match path_exists {
            Some(true) => consequential("saves over a file that already exists".into()),
            Some(false) => StepClass::Routine,
            None => unclassifiable(
                "could not resolve the save path to check whether the file already exists".into(),
            ),
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
    if tokens.is_empty() {
        return unclassifiable("targets an element with no readable name".into());
    }
    let shown = escape_text(name.trim());
    let risky_word = tokens
        .iter()
        .find(|token| RISKY_NAME_WORDS.iter().any(|word| token.starts_with(word)));
    if let Some(word) = risky_word {
        return consequential(format!("targets an element named \"{shown}\" ({word})"));
    }
    let sign_out = tokens
        .windows(2)
        .any(|pair| pair[0] == "sign" && pair[1].starts_with("out"))
        || tokens.iter().any(|token| token == "signout");
    if sign_out {
        return consequential(format!("targets an element named \"{shown}\" (sign out)"));
    }
    if in_confirmation_dialog && tokens.len() == 1 && CONFIRM_NAMES.contains(&tokens[0].as_str()) {
        return consequential(format!("confirms a dialog with \"{shown}\""));
    }
    StepClass::Routine
}

/// Split a key sequence into chords of normalized key names.
///
/// Chords are separated by whitespace or commas; keys within a chord by `+` or `-`, in any case
/// and order (`Control` is `ctrl`, `Del` is `delete`). `None` when the text is malformed.
fn parse_chords(keys: &str) -> Option<Vec<Vec<String>>> {
    let chars: Vec<char> = keys.trim().chars().collect();
    let is_separator = |c: char| c == '+' || c == '-';
    let mut compact = String::new();
    for (index, &c) in chars.iter().enumerate() {
        let touches_separator = |other: Option<&char>| other.is_some_and(|c| is_separator(*c));
        let previous = chars[..index].iter().rev().find(|c| !c.is_whitespace());
        let next = chars[index + 1..].iter().find(|c| !c.is_whitespace());
        if c.is_whitespace() && (touches_separator(previous) || touches_separator(next)) {
            continue;
        }
        compact.push(if c == ',' { ' ' } else { c });
    }
    let chords: Vec<Vec<String>> = compact
        .split_whitespace()
        .map(|chord| {
            chord
                .split(is_separator)
                .map(|part| match part.to_lowercase().as_str() {
                    "control" => "ctrl".to_owned(),
                    "del" => "delete".to_owned(),
                    other => other.to_owned(),
                })
                .collect()
        })
        .collect();
    let malformed = chords.is_empty() || chords.iter().flatten().any(String::is_empty);
    (!malformed).then_some(chords)
}

fn classify_press(keys: &str, focused: Option<&str>, focused_in_dialog: bool) -> StepClass {
    let shown = escape_text(keys.trim());
    let Some(chords) = parse_chords(keys) else {
        return unclassifiable(format!("key sequence `{shown}` is malformed"));
    };
    for (index, chord) in chords.iter().enumerate() {
        let holds = |keys: &[&str]| keys.iter().all(|key| chord.iter().any(|part| part == key));
        if RISKY_KEYS.iter().any(|risky| holds(risky)) {
            return consequential(format!("presses {shown}"));
        }
        let activates = chord
            .iter()
            .any(|part| ACTIVATING_KEYS.contains(&part.as_str()))
            || chord.as_slice() == ["space"];
        if !activates {
            continue;
        }
        if index > 0 {
            return unclassifiable(format!(
                "presses {shown}; focus may have changed before the activating key"
            ));
        }
        let Some(name) = focused else {
            return unclassifiable(format!(
                "presses {shown}; the focused element is unknown or ambiguous"
            ));
        };
        if let StepClass::NeedsApproval(flag) = classify_element(name, focused_in_dialog) {
            return StepClass::NeedsApproval(StepFlag {
                reason: format!("presses {shown} and {}", flag.reason),
                ..flag
            });
        }
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
