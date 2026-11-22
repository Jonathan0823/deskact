//! Fixture tests for the agent-loop tool set confined to one target window.

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use deskact_core::{NativeWindow, OBSERVATION_BYTE_CAP, TargetWindowTools, VerifiedNativeCli};

struct Fixture {
    dir: PathBuf,
}

const SNAPSHOT: &str = r##"{"window":{"id":"fixture-window"},"complete":true,"ref_count":4,"snapshot_id":"s1","tree":{"role":"window","name":"Untitled - Notepad","children":[
  {"role":"textfield","name":"Text editor","ref_id":"@e1","value":"hello","states":["focused"]},
  {"role":"button","name":"Close","ref_id":"@e2"},
  {"role":"dialog","name":"Save As","children":[
    {"role":"textfield","name":"File name","ref_id":"@e3"},
    {"role":"button","name":"Save","ref_id":"@e4"}]}]}}"##;

impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "deskact-agent-tools-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create fixture directory");
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_agent_desktop.rs");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = Command::new(rustc)
            .arg("--edition=2024")
            .arg(source)
            .arg("-o")
            .arg(dir.join("agent-desktop"))
            .output()
            .expect("compile fake agent-desktop");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::write(dir.join("calls.log"), "").expect("create call log");
        fs::write(dir.join("snapshot.json"), SNAPSHOT).expect("write snapshot");
        Self { dir }
    }

    fn scenario(&self, name: &str, content: &str) {
        fs::write(self.dir.join(name), content).expect("write scenario");
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("calls.log"))
            .expect("read calls")
            .lines()
            .filter(|line| !line.starts_with("version"))
            .map(str::to_owned)
            .collect()
    }

    fn tools(&self) -> TargetWindowTools {
        let cli = VerifiedNativeCli::connect_to(self.dir.join("agent-desktop")).expect("connect");
        let window = NativeWindow {
            id: "fixture-window".into(),
            title: "Untitled - Notepad".into(),
            app_name: "notepad".into(),
            accessible: true,
            minimized: false,
            is_focused: false,
            visible: true,
            pid: 4512,
        };
        TargetWindowTools::new(cli, window)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// Observation is compact and includes controls of an owned dialog.
#[test]
fn observe_returns_compact_snapshot_including_owned_dialog() {
    let fixture = Fixture::new();
    let mut tools = fixture.tools();

    let observation = tools.observe(None).expect("observe");

    assert!(!observation.truncated);
    assert!(observation.text.contains("Save As"));
    assert!(observation.text.contains("@e4"));
    assert!(!observation.text.contains("snapshot_id"));
    assert_eq!(
        fixture.calls(),
        ["snapshot --app notepad --window-id fixture-window -i"]
    );
}

/// Observation stays within the byte cap and reports truncation.
#[test]
fn observe_truncates_to_the_byte_cap() {
    let fixture = Fixture::new();
    let nodes: Vec<String> = (1..=2000)
        .map(|n| format!(r#"{{"role":"button","name":"Button number {n}","ref_id":"@e{n}"}}"#))
        .collect();
    fixture.scenario(
        "snapshot.json",
        &format!(
            r#"{{"window":{{"id":"fixture-window"}},"complete":true,"ref_count":2000,"snapshot_id":"s","tree":{{"role":"window","children":[{}]}}}}"#,
            nodes.join(",")
        ),
    );
    let mut tools = fixture.tools();

    let observation = tools.observe(None).expect("observe");

    assert!(observation.truncated);
    assert!(observation.text.len() <= OBSERVATION_BYTE_CAP);
    assert!(observation.text.contains(r#""@e1""#));
    assert!(
        tools.click("@e2000").is_err(),
        "unseen refs stay unreachable"
    );
}

/// Drill-down roots must come from an earlier observation of the target window.
#[test]
fn observe_drill_down_requires_an_observed_root() {
    let fixture = Fixture::new();
    let mut tools = fixture.tools();

    assert!(tools.observe(Some("@e3")).is_err());
    tools.observe(None).expect("observe");
    tools.observe(Some("@e3")).expect("drill down");

    assert_eq!(
        fixture.calls().last().expect("call"),
        "snapshot --app notepad --window-id fixture-window -i --root @e3"
    );
}

/// Refs never shown for the target window are rejected without reaching the CLI.
#[test]
fn refs_outside_the_target_window_are_rejected() {
    let fixture = Fixture::new();
    let mut tools = fixture.tools();

    assert!(tools.click("@e1").is_err(), "no observation yet");
    tools.observe(None).expect("observe");
    let before = fixture.calls().len();

    assert!(tools.click("@e99").is_err());
    assert!(tools.set_value("@e99", "x").is_err());
    assert!(tools.read_value("@e99").is_err());
    assert_eq!(fixture.calls().len(), before);
}

/// Click, set_value and read_value address observed refs through agent-desktop, including dialogs.
#[test]
fn click_set_value_and_read_value_use_observed_refs() {
    let fixture = Fixture::new();
    fixture.scenario("value.json", r#""hello""#);
    let mut tools = fixture.tools();
    tools.observe(None).expect("observe");

    tools.set_value("@e1", "new text").expect("set");
    tools.click("@e4").expect("dialog button");
    let value = tools.read_value("@e1").expect("read");

    assert_eq!(value, "hello");
    assert_eq!(
        &fixture.calls()[1..],
        [
            "set-value @e1 new text",
            "click @e4",
            "get @e1 --property value"
        ]
    );
}

/// Keys are sent only after focus on the target window is confirmed.
#[test]
fn press_focuses_and_confirms_before_sending_keys() {
    let fixture = Fixture::new();
    let tools = fixture.tools();

    tools.press("ctrl+s").expect("press");

    assert_eq!(
        fixture.calls(),
        [
            "focus-window --app notepad --window-id fixture-window",
            "press --headed --app notepad --window-id fixture-window ctrl+s"
        ]
    );
}

/// AT-50: unconfirmed focus stops safely without sending any keystroke.
#[test]
fn press_sends_nothing_when_focus_is_not_confirmed() {
    let fixture = Fixture::new();
    fixture.scenario("focused.json", "false");
    let tools = fixture.tools();

    let error = tools.press("ctrl+s").expect_err("focus unconfirmed");

    assert!(error.to_string().contains("confirm focus"));
    assert!(
        fixture
            .calls()
            .iter()
            .all(|call| !call.starts_with("press"))
    );
}

/// Empty or flag-like key strings are refused before touching the CLI.
#[test]
fn press_rejects_malformed_keys() {
    let fixture = Fixture::new();
    let tools = fixture.tools();

    assert!(tools.press("").is_err());
    assert!(tools.press("--help").is_err());
    assert!(fixture.calls().is_empty());
}
