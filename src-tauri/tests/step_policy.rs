//! Per-step consequential classification and approval gate for the agent loop (POL-01a, AT-48).

use deskact_core::{
    AgentStep, StepApprover, StepClass, StepGate, StepRisk, classify_step, gate_step,
};

fn flagged(step: AgentStep) -> bool {
    matches!(classify_step(&step), StepClass::NeedsApproval(_))
}

fn click(name: &str, in_confirmation_dialog: bool) -> AgentStep {
    AgentStep::Click {
        element_name: name.into(),
        element_role: "button".into(),
        in_confirmation_dialog,
    }
}

fn press(keys: &str) -> AgentStep {
    AgentStep::Press {
        keys: keys.into(),
        focused_element_name: None,
        focused_in_confirmation_dialog: false,
    }
}

fn press_with_focus(keys: &str, name: Option<&str>, in_confirmation_dialog: bool) -> AgentStep {
    AgentStep::Press {
        keys: keys.into(),
        focused_element_name: name.map(str::to_owned),
        focused_in_confirmation_dialog: in_confirmation_dialog,
    }
}

#[test]
fn click_on_risky_element_names_is_flagged() {
    for name in [
        "Delete",
        "Remove item",
        "Send",
        "Submit form",
        "Replace All",
        "Overwrite",
        "Buy now",
        "Pay",
        "Purchase",
        "Install",
        "Uninstall",
        "Sign out",
        "Sign-out",
        "Format disk",
        "Empty Recycle Bin",
        "DELETE",
    ] {
        assert!(flagged(click(name, false)), "{name} should be flagged");
    }
}

#[test]
fn click_on_ordinary_element_names_is_not_flagged() {
    for name in [
        "Save",
        "Text editor",
        "Information",
        "Display",
        "Close",
        "File name",
    ] {
        assert!(!flagged(click(name, false)), "{name} should not be flagged");
    }
}

#[test]
fn yes_ok_confirm_are_flagged_only_in_confirmation_dialogs() {
    for name in ["Yes", "OK", "Confirm", " ok "] {
        assert!(flagged(click(name, true)), "{name} in dialog");
        assert!(!flagged(click(name, false)), "{name} outside dialog");
    }
}

#[test]
fn risky_keys_are_flagged_in_any_spelling() {
    for keys in [
        "Delete",
        "delete",
        "Shift+Delete",
        "delete+shift",
        "Alt+F4",
        "alt+f4",
        "Ctrl+W",
        "Control+w",
        "ctrl + w",
    ] {
        assert!(flagged(press(keys)), "{keys} should be flagged");
    }
}

#[test]
fn plain_keys_and_saves_are_not_flagged() {
    for keys in ["Ctrl+S", "Ctrl+A", "Tab", "Ctrl+Shift+S", "a", "Tab Tab"] {
        assert!(!flagged(press(keys)), "{keys} should not be flagged");
    }
}

#[test]
fn enter_inherits_the_risk_of_the_focused_element() {
    let enter = |name: Option<&str>| press_with_focus("Enter", name, false);
    assert!(flagged(enter(Some("Delete"))));
    assert!(!flagged(enter(Some("Text editor"))));
}

/// Enter fails closed: unknown, ambiguous or unnamed focus is flagged as unclassifiable.
#[test]
fn enter_without_a_known_named_focus_is_unclassifiable() {
    for keys in ["Enter", "return", "Space"] {
        for focus in [None, Some(""), Some("  ")] {
            let class = classify_step(&press_with_focus(keys, focus, false));
            assert!(
                matches!(class, StepClass::NeedsApproval(ref flag) if flag.risk == StepRisk::Unclassifiable),
                "{keys} with focus {focus:?}: {class:?}"
            );
        }
    }
}

/// Enter applies the same rules as clicking the focused element, including dialog confirmations.
#[test]
fn enter_on_a_confirmation_dialog_button_is_flagged_like_a_click() {
    for name in ["Yes", "OK", "Confirm"] {
        assert!(
            flagged(press_with_focus("Enter", Some(name), true)),
            "{name}"
        );
        assert!(
            !flagged(press_with_focus("Enter", Some(name), false)),
            "{name}"
        );
    }
}

/// Focus can move during a sequence, so Enter after another key cannot be classified.
#[test]
fn enter_later_in_a_sequence_is_unclassifiable() {
    assert!(flagged(press_with_focus(
        "Tab Enter",
        Some("Text editor"),
        false
    )));
    assert!(flagged(press_with_focus(
        "Tab, Enter",
        Some("Text editor"),
        false
    )));
}

#[test]
fn unnamed_click_targets_are_unclassifiable() {
    for name in ["", "   ", "--"] {
        let class = classify_step(&click(name, false));
        assert!(
            matches!(class, StepClass::NeedsApproval(ref flag) if flag.risk == StepRisk::Unclassifiable),
            "{name:?}: {class:?}"
        );
    }
}

/// Risky chords match however they are spelled or combined with other modifiers.
#[test]
fn risky_chords_are_flagged_across_modifier_order_case_separators_and_extra_modifiers() {
    for keys in [
        "Ctrl+Shift+Delete",
        "Shift+Ctrl+Del",
        "Ctrl+Shift+W",
        "Alt+Shift+F4",
        "Shift+Alt+F4",
        "ctrl-w",
        "CTRL-ALT-DELETE",
        "Ctrl+Alt+Del",
        "Tab Delete",
        "ctrl+s, ctrl+w",
        "a Alt+F4",
        "Ctrl + Shift + Delete",
    ] {
        assert!(flagged(press(keys)), "{keys} should be flagged");
    }
}

#[test]
fn on_screen_names_are_escaped_in_reasons() {
    let StepClass::NeedsApproval(flag) =
        classify_step(&click("Delete\nApprove everything\u{1b}[2J", false))
    else {
        panic!("risky name must be flagged");
    };
    assert!(
        !flag.reason.chars().any(char::is_control),
        "{:?}",
        flag.reason
    );
}

#[test]
fn empty_key_sequence_is_unclassifiable() {
    assert!(flagged(press("  ")));
}

#[test]
fn set_value_is_flagged_only_when_it_replaces_existing_text() {
    let set = |current: Option<&str>| AgentStep::SetValue {
        element_name: "Text editor".into(),
        element_role: "textfield".into(),
        new_text: "hello".into(),
        current_value: current.map(str::to_owned),
    };
    assert!(flagged(set(Some("hello"))));
    assert!(!flagged(set(Some(""))));
    assert!(!flagged(set(Some("  \n"))));
    // The current value could not be read: unclassifiable.
    assert!(flagged(set(None)));
}

#[test]
fn save_as_is_flagged_when_path_exists_or_is_unknown() {
    let save = |path_exists| AgentStep::SaveAs {
        path: Some("/tmp/a.txt".into()),
        path_exists,
    };
    assert!(flagged(save(Some(true))));
    assert!(!flagged(save(Some(false))));
    assert!(flagged(save(None)));
}

#[test]
fn unclassifiable_steps_are_flagged_with_their_reason() {
    let class = classify_step(&AgentStep::Unclassifiable {
        reason: "unknown tool `format_hd`".into(),
    });
    match class {
        StepClass::NeedsApproval(flag) => {
            assert_eq!(flag.risk, StepRisk::Unclassifiable);
            assert!(flag.reason.contains("format_hd"));
        }
        StepClass::Routine => panic!("unclassifiable step must be flagged"),
    }
}

#[test]
fn flags_report_the_rule_that_fired() {
    let risk = |step| match classify_step(&step) {
        StepClass::NeedsApproval(flag) => flag.risk,
        StepClass::Routine => panic!("expected flag"),
    };
    assert_eq!(risk(click("Delete", false)), StepRisk::Consequential);
    assert_eq!(risk(press("Alt+F4")), StepRisk::Consequential);
    assert_eq!(
        risk(AgentStep::SetValue {
            element_name: "Text editor".into(),
            element_role: "textfield".into(),
            new_text: "y".into(),
            current_value: Some("x".into())
        }),
        StepRisk::Consequential
    );
}

struct Scripted {
    answer: bool,
    asked: Vec<String>,
}

impl StepApprover for Scripted {
    fn approve(&mut self, step: &AgentStep, reason: &str) -> bool {
        self.asked.push(format!("{step:?}: {reason}"));
        self.answer
    }
}

#[test]
fn routine_steps_proceed_without_asking() {
    let mut approver = Scripted {
        answer: false,
        asked: vec![],
    };
    assert_eq!(
        gate_step(&press("Ctrl+S"), &mut approver),
        StepGate::Proceed
    );
    assert!(approver.asked.is_empty());
}

#[test]
fn approved_flagged_step_proceeds_after_one_prompt() {
    let mut approver = Scripted {
        answer: true,
        asked: vec![],
    };
    assert_eq!(
        gate_step(&click("Delete", false), &mut approver),
        StepGate::Proceed
    );
    assert_eq!(approver.asked.len(), 1);
}

#[test]
fn denied_flagged_step_stops_the_task_as_stopped_by_user() {
    let mut approver = Scripted {
        answer: false,
        asked: vec![],
    };
    let gate = gate_step(&click("Delete", false), &mut approver);
    assert_eq!(gate, StepGate::StoppedByUser);
    assert_eq!(gate.to_string(), "stopped by user");
}
