#![cfg(unix)]

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

const DEFAULT_CATALOG: &str = r#"{"entries":[{"name":"Calculator","target":"C:\\Apps\\calculator.exe","arguments":"","working_directory":"","source":"start-menu"}],"skipped_start_menu_shortcuts":0}"#;

struct Fixture {
    dir: PathBuf,
    cli: PathBuf,
    powershell: PathBuf,
    calls: PathBuf,
    catalog: String,
    process_path: String,
}

impl Fixture {
    fn new() -> Self {
        Self::with_catalog(DEFAULT_CATALOG)
    }

    fn with_catalog(catalog: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("deskact-resolve-{}-{unique}", std::process::id()));
        fs::create_dir_all(&dir).expect("create fixture directory");
        let calls = dir.join("calls.log");
        let cli = dir.join("agent-desktop");
        fs::write(
            &cli,
            r##"#!/bin/sh
printf '%s\n' "$*" >> "$DESKACT_TEST_CALLS"
case "$1" in
  version) printf '%s\n' '{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}' ;;
  launch) printf '%s\n' '{"version":"2.6","ok":true,"command":"launch","data":{"app":"C:\\Apps\\calculator.exe","pid":4001}}' ;;
  list-windows) n=$(grep -c '^list-windows' "$DESKACT_TEST_CALLS"); response=$(printf '%s\n' "$DESKACT_TEST_WINDOWS" | sed -n "${n}p"); if [ -z "$response" ]; then response=$(printf '%s\n' "$DESKACT_TEST_WINDOWS" | tail -n 1); fi; printf '%s\n' "$response" ;;
  snapshot) printf '%s\n' "$DESKACT_TEST_SNAPSHOT" ;;
  *) echo "unexpected command: $1" >&2; exit 2 ;;
esac
"##,
        )
        .expect("write fake CLI");
        let mut permissions = fs::metadata(&cli)
            .expect("read fake CLI metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&cli, permissions).expect("make fake CLI executable");

        let powershell = dir.join("powershell");
        fs::write(
            &powershell,
            r#"#!/bin/sh
case "$*" in
  *Win32_Process*) printf '%s\n' "$DESKACT_TEST_PROCESS_PATH" ;;
  *) printf '%s\n' "$DESKACT_TEST_CATALOG" ;;
esac
"#,
        )
        .expect("write fixture PowerShell");
        let mut permissions = fs::metadata(&powershell)
            .expect("read fixture PowerShell metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&powershell, permissions).expect("make fixture PowerShell executable");

        Self {
            dir,
            cli,
            powershell,
            calls,
            catalog: catalog.to_owned(),
            process_path: r"C:\Apps\calculator.exe".to_owned(),
        }
    }

    fn run(&self, app_name: &str, windows: &str, snapshot: &str) -> Output {
        self.run_with_args(&["resolve", app_name], windows, snapshot)
    }

    fn run_with_args(&self, args: &[&str], windows: &str, snapshot: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .args(args)
            .env("DESKACT_AGENT_DESKTOP", &self.cli)
            .env("DESKACT_POWERSHELL", &self.powershell)
            .env("DESKACT_TEST_CALLS", &self.calls)
            .env("DESKACT_TEST_WINDOWS", windows)
            .env("DESKACT_TEST_SNAPSHOT", snapshot)
            .env("DESKACT_TEST_CATALOG", &self.catalog)
            .env("DESKACT_APP_CATALOG", self.dir.join("app-catalog.json"))
            .env("DESKACT_TEST_PROCESS_PATH", &self.process_path)
            .output()
            .expect("run developer harness")
    }

    fn run_with_input(&self, app_name: &str, windows: &str, snapshot: &str, input: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .args(["resolve", app_name])
            .env("DESKACT_AGENT_DESKTOP", &self.cli)
            .env("DESKACT_POWERSHELL", &self.powershell)
            .env("DESKACT_TEST_CALLS", &self.calls)
            .env("DESKACT_TEST_WINDOWS", windows)
            .env("DESKACT_TEST_SNAPSHOT", snapshot)
            .env("DESKACT_TEST_CATALOG", &self.catalog)
            .env("DESKACT_APP_CATALOG", self.dir.join("app-catalog.json"))
            .env("DESKACT_TEST_PROCESS_PATH", &self.process_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run developer harness");
        child
            .stdin
            .take()
            .expect("stdin is piped")
            .write_all(input.as_bytes())
            .expect("write user response");
        child
            .wait_with_output()
            .expect("wait for developer harness")
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(&self.calls)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn resolve_launches_then_uses_a_fresh_unique_accessible_window_and_snapshot() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false,"visible":true,"pid":9002}]}"#;
    let snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"calculator","window":{"id":"window-7","title":"Calculator"},"snapshot_id":"snap-1","ref_count":1,"complete":true,"tree":{"role":"window","name":"Calculator","children":[{"role":"button","name":"7","ref_id":"@snap-1:e1"}]}}}"#;

    let output = fixture.run("Calculator", windows, snapshot);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}"
    );
    assert!(stdout.contains("Calculator"), "{stdout}");
    assert!(stdout.contains("window-7"), "{stdout}");
    assert!(stdout.contains("snap-1"), "{stdout}");
    let calls = fixture.calls();
    assert_eq!(calls[0..2], ["version", "launch C:\\Apps\\calculator.exe"]);
    assert!(calls[2].starts_with("list-windows --timeout-ms "));
    assert_eq!(
        calls[3],
        "snapshot --app calculator --window-id window-7 -i"
    );
}

#[test]
fn resolve_fails_closed_when_more_than_one_accessible_window_matches() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false,"pid":9002},{"id":"window-8","title":"Calculator 2","app_name":"calculator","accessible":true,"minimized":false,"pid":9003}]}"#;
    let output = fixture.run("Calculator", windows, "");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("exactly one is required"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fixture
            .calls()
            .last()
            .unwrap()
            .starts_with("list-windows --timeout-ms ")
    );
}

#[test]
fn resolve_fails_closed_for_an_inaccessible_window() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":false,"minimized":false}]}"#;
    let output = fixture.run("Calculator", windows, "");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("No matching window"));
    assert!(
        fixture
            .calls()
            .last()
            .unwrap()
            .starts_with("list-windows --timeout-ms ")
    );
}

#[test]
fn resolve_waits_for_manual_restore_then_reenumerates_without_relaunching() {
    let fixture = Fixture::new();
    let minimized = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":true,"visible":false,"pid":4001}]}"#;
    let restored = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false,"visible":true,"pid":4001}]}"#;
    let snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"calculator","window":{"id":"window-7","title":"Calculator"},"snapshot_id":"snap-2","ref_count":1,"complete":true,"tree":{"role":"window","name":"Calculator"}}}"#;
    let output = fixture.run_with_input(
        "Calculator",
        &format!("{minimized}\n{restored}"),
        snapshot,
        "\n",
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("is minimized"), "{stdout}");
    assert!(stdout.contains("snap-2"), "{stdout}");
    let calls = fixture.calls();
    assert_eq!(calls[0..2], ["version", "launch C:\\Apps\\calculator.exe"]);
    assert!(calls[2].starts_with("list-windows --timeout-ms "));
    assert_eq!(
        &calls[3..],
        [
            "version",
            "list-windows",
            "snapshot --app calculator --window-id window-7 -i",
        ]
    );
}

#[test]
fn resolve_fails_closed_when_fresh_inventory_has_no_matching_window() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[]}"#;
    let output = fixture.run("Calculator", windows, "");

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("No fresh window matched"),
        "{} calls: {:?}",
        String::from_utf8_lossy(&output.stderr),
        fixture.calls()
    );
    assert!(
        fixture
            .calls()
            .last()
            .unwrap()
            .starts_with("list-windows --timeout-ms ")
    );
}

#[test]
fn resolve_fails_closed_when_accessibility_snapshot_is_incomplete() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false,"pid":9002}]}"#;
    let snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"calculator","window":{"id":"window-7","title":"Calculator"},"snapshot_id":"snap-partial","ref_count":1,"complete":false,"tree":{"role":"window","name":"Calculator"}}}"#;
    let output = fixture.run("Calculator", windows, snapshot);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("snapshot for `Calculator` was incomplete"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.calls().last().unwrap().starts_with("snapshot "));
}

#[test]
fn resolve_polls_for_window_readiness_without_relaunching() {
    let fixture = Fixture::new();
    let absent = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[]}"#;
    let available = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false,"pid":9002}]}"#;
    let snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"calculator","window":{"id":"window-7","title":"Calculator"},"snapshot_id":"snap-ready","ref_count":1,"complete":true,"tree":{"role":"window","name":"Calculator"}}}"#;
    let output = fixture.run("Calculator", &format!("{absent}\n{available}"), snapshot);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("snap-ready"));
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|call| call.starts_with("launch "))
            .count(),
        1
    );
    assert_eq!(
        fixture
            .calls()
            .iter()
            .filter(|call| call.starts_with("list-windows "))
            .count(),
        2
    );
}

#[test]
fn resolve_fails_closed_when_owner_image_does_not_match_target_executable() {
    let mut fixture = Fixture::new();
    fixture.process_path = r"C:\Windows\ApplicationFrameHost.exe".to_owned();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false,"pid":9002}]}"#;
    let output = fixture.run("Calculator", windows, "");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("hosted-app/container window"), "{stderr}");
    assert!(
        fixture
            .calls()
            .iter()
            .all(|call| !call.starts_with("snapshot "))
    );
}

#[test]
fn resolve_fails_when_snapshot_reports_a_different_window_id() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false,"pid":9002}]}"#;
    let snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"calculator","window":{"id":"window-8","title":"Other window"},"snapshot_id":"snap-other","ref_count":1,"complete":true,"tree":{"role":"window","name":"Other window"}}}"#;
    let output = fixture.run("Calculator", windows, snapshot);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("did not confirm the selected window `window-7`")
    );
}

#[test]
fn resolve_rejects_shortcut_arguments_without_launching() {
    let catalog = r#"{"entries":[{"name":"Calculator","target":"C:\\Apps\\calculator.exe","arguments":"--profile test","working_directory":"","source":"start-menu"}],"skipped_start_menu_shortcuts":0}"#;
    let fixture = Fixture::with_catalog(catalog);
    let output = fixture.run("Calculator", "", "");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("does not yet parse safely"));
    assert!(
        fixture
            .calls()
            .iter()
            .all(|call| !call.starts_with("launch "))
    );
}

#[test]
fn resolve_matches_non_ascii_display_names_without_ascii_case_folding() {
    let catalog = r#"{"entries":[{"name":"Straße","target":"C:\\Apps\\calculator.exe","arguments":"","working_directory":"","source":"start-menu"}],"skipped_start_menu_shortcuts":0}"#;
    let fixture = Fixture::with_catalog(catalog);
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false,"pid":9002}]}"#;
    let snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"calculator","window":{"id":"window-7","title":"Calculator"},"snapshot_id":"snap-unicode","ref_count":1,"complete":true,"tree":{"role":"window","name":"Calculator"}}}"#;
    let output = fixture.run("STRAẞE", windows, snapshot);

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("snap-unicode"));
}

#[test]
fn resolve_rejects_entries_with_distinct_working_directories_as_ambiguous() {
    let catalog = r#"{"entries":[{"name":"Calculator","target":"C:\\Apps\\calculator.exe","arguments":"","working_directory":"C:\\First","source":"start-menu"},{"name":"Calculator","target":"C:\\Apps\\calculator.exe","arguments":"","working_directory":"C:\\Second","source":"app-paths"}],"skipped_start_menu_shortcuts":0}"#;
    let fixture = Fixture::with_catalog(catalog);
    let output = fixture.run("Calculator", "", "");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exactly matches"));
    assert!(
        fixture
            .calls()
            .iter()
            .all(|call| !call.starts_with("launch "))
    );
}

#[test]
fn resolve_rejects_missing_or_extra_positional_arguments_before_resolution() {
    let fixture = Fixture::new();
    for args in [&["resolve"][..], &["resolve", "Calculator", "extra"][..]] {
        let output = fixture.run_with_args(args, "", "");
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("Usage: deskact-dev resolve"));
    }
    assert!(fixture.calls().is_empty());
}
