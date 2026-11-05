#![cfg(unix)]

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    dir: PathBuf,
    cli: PathBuf,
    powershell: PathBuf,
    calls: PathBuf,
}

impl Fixture {
    fn new() -> Self {
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
  list-windows) n=$(grep -c '^list-windows' "$DESKACT_TEST_CALLS"); printf '%s\n' "$DESKACT_TEST_WINDOWS" | sed -n "${n}p" ;;
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
printf '%s\n' '{"entries":[{"name":"Calculator","target":"C:\\Apps\\calculator.exe","arguments":"","working_directory":"","source":"start-menu"}],"skipped_start_menu_shortcuts":0}'
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
        }
    }

    fn run(&self, app_name: &str, windows: &str, snapshot: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .args(["resolve", app_name])
            .env("DESKACT_AGENT_DESKTOP", &self.cli)
            .env("DESKACT_POWERSHELL", &self.powershell)
            .env("DESKACT_TEST_CALLS", &self.calls)
            .env("DESKACT_TEST_WINDOWS", windows)
            .env("DESKACT_TEST_SNAPSHOT", snapshot)
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
            .expect("read CLI calls")
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
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false,"visible":true,"pid":4001}]}"#;
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
    assert_eq!(
        fixture.calls(),
        [
            "version",
            "launch C:\\Apps\\calculator.exe",
            "list-windows",
            "snapshot --app calculator --window-id window-7 -i",
        ]
    );
}

#[test]
fn resolve_fails_closed_when_more_than_one_accessible_window_matches() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false},{"id":"window-8","title":"Calculator 2","app_name":"calculator","accessible":true,"minimized":false}]}"#;
    let output = fixture.run("Calculator", windows, "");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exactly one is required"));
    assert_eq!(
        fixture.calls().last().map(String::as_str),
        Some("list-windows")
    );
}

#[test]
fn resolve_fails_closed_for_an_inaccessible_window() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":false,"minimized":false}]}"#;
    let output = fixture.run("Calculator", windows, "");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("No matching window"));
    assert_eq!(
        fixture.calls().last().map(String::as_str),
        Some("list-windows")
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
    assert_eq!(
        fixture.calls(),
        [
            "version",
            "launch C:\\Apps\\calculator.exe",
            "list-windows",
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
    assert!(String::from_utf8_lossy(&output.stderr).contains("No fresh window matched"));
    assert_eq!(
        fixture.calls().last().map(String::as_str),
        Some("list-windows")
    );
}

#[test]
fn resolve_fails_closed_when_accessibility_snapshot_is_incomplete() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"window-7","title":"Calculator","app_name":"calculator","accessible":true,"minimized":false}]}"#;
    let snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"calculator","window":{"id":"window-7","title":"Calculator"},"snapshot_id":"snap-partial","ref_count":1,"complete":false,"tree":{"role":"window","name":"Calculator"}}}"#;
    let output = fixture.run("Calculator", windows, snapshot);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("snapshot for `Calculator` was incomplete")
    );
    assert!(fixture.calls().last().unwrap().starts_with("snapshot "));
}
