#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture {
    dir: PathBuf,
    cli: PathBuf,
    powershell: PathBuf,
    calls: PathBuf,
    marker: PathBuf,
    filename: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("deskact-notepad-{}-{unique}", std::process::id()));
        fs::create_dir_all(&dir).expect("create fixture directory");
        let cli = dir.join("agent-desktop");
        let powershell = dir.join("powershell");
        let calls = dir.join("calls.log");
        let marker = dir.join("marker.txt");
        let filename = dir.join("filename.txt");
        fs::write(
            &cli,
            r##"#!/bin/sh
printf '%s\n' "$*" >> "$DESKACT_TEST_CALLS"
case "$1" in
  version) printf '%s\n' '{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}' ;;
  list-windows) n=$(grep -c '^list-windows' "$DESKACT_TEST_CALLS"); file=$(cat "$DESKACT_TEST_FILENAME" 2>/dev/null); if grep -q 'ctrl+w' "$DESKACT_TEST_CALLS"; then response=$(printf '%s\n' "$DESKACT_TEST_WINDOWS" | tail -n 1); printf '%s\n' "$response"; elif [ -n "$file" ] && [ -f "$file" ]; then name=$(basename "$file"); printf '{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"user-window","title":"%s - Notepad","app_name":"notepad","accessible":true,"minimized":false,"visible":true,"pid":9876}]}\n' "$name"; elif grep -q 'ctrl+s' "$DESKACT_TEST_CALLS"; then response=$(printf '%s\n' "$DESKACT_TEST_WINDOWS" | sed -n '3p'); printf '%s\n' "$response"; elif grep -q 'ctrl+n' "$DESKACT_TEST_CALLS"; then if [ "$n" -eq 2 ]; then response=$(printf '%s\n' "$DESKACT_TEST_WINDOWS" | sed -n '1p'); else response=$(printf '%s\n' "$DESKACT_TEST_WINDOWS" | sed -n '2p'); fi; printf '%s\n' "$response"; else response=$(printf '%s\n' "$DESKACT_TEST_WINDOWS" | sed -n "${n}p"); if [ -z "$response" ]; then response=$(printf '%s\n' "$DESKACT_TEST_WINDOWS" | tail -n 1); fi; printf '%s\n' "$response"; fi ;;
  press) printf '%s\n' '{"version":"2.6","ok":true,"command":"press","data":{"disposition":"delivered_unverified"}}' ;;
  snapshot) case "$*" in *save-dialog*) path=$(cat "$DESKACT_TEST_FILENAME"); printf '{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"Notepad.exe","window":{"id":"save-dialog","title":"Save as"},"snapshot_id":"snap-dialog","ref_count":2,"complete":true,"tree":{"role":"window","children":[{"role":"textfield","name":"File name:","ref_id":"@snap-dialog:e1","value":"%s"},{"role":"button","name":"Save","ref_id":"@snap-dialog:e2"}]}}}\n' "$path" ;; *) n=$(grep -c '^snapshot ' "$DESKACT_TEST_CALLS"); if [ "$n" -eq 1 ]; then printf '%s\n' "$DESKACT_TEST_NEW_SNAPSHOT"; else printf '%s\n' "$DESKACT_TEST_SAVED_SNAPSHOT"; fi ;; esac ;;
  set-value) if [ "$2" = '@snap-new:e1' ]; then printf '%s' "$3" > "$DESKACT_TEST_MARKER"; else printf '%s' "$3" > "$DESKACT_TEST_FILENAME"; fi; printf '%s\n' '{"version":"2.6","ok":true,"command":"set-value","data":{"disposition":"delivered_unverified"}}' ;;
  click) n=$(grep -c '^click ' "$DESKACT_TEST_CALLS"); if [ "$n" -ge 2 ]; then file=$(cat "$DESKACT_TEST_FILENAME"); cp "$DESKACT_TEST_MARKER" "$file"; fi; printf '%s\n' '{"version":"2.6","ok":true,"command":"click","data":{"disposition":"delivered_unverified"}}' ;;
  get) value=$(cat "$DESKACT_TEST_MARKER" 2>/dev/null); printf '{"version":"2.6","ok":true,"command":"get","data":{"property":"value","value":"%s"}}\n' "$value" ;;
  *) echo "unexpected command: $1" >&2; exit 2 ;;
esac
"##,
        )
        .expect("write fake native CLI");
        make_executable(&cli);
        fs::write(
            &powershell,
            r##"#!/bin/sh
case "$*" in
  *Win32_Process*) printf '%s\n' 'C:\Windows\System32\notepad.exe' ;;
  *) printf '%s\n' '{"entries":[{"name":"Notepad","target":"C:\\Windows\\System32\\notepad.exe","arguments":"","working_directory":"","source":"app-paths"}],"skipped_start_menu_shortcuts":0}' ;;
esac
"##,
        )
        .expect("write fixture PowerShell");
        make_executable(&powershell);
        Self {
            dir,
            cli,
            powershell,
            calls,
            marker,
            filename,
        }
    }

    fn run(
        &self,
        windows: &str,
        new_snapshot: &str,
        save_snapshot: &str,
        saved_snapshot: &str,
    ) -> std::process::Output {
        self.run_count(1, windows, new_snapshot, save_snapshot, saved_snapshot)
    }

    fn run_count(
        &self,
        runs: usize,
        windows: &str,
        new_snapshot: &str,
        save_snapshot: &str,
        saved_snapshot: &str,
    ) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .args(["notepad-acceptance", "--runs", &runs.to_string()])
            .env("DESKACT_AGENT_DESKTOP", &self.cli)
            .env("DESKACT_POWERSHELL", &self.powershell)
            .env("DESKACT_TEST_CALLS", &self.calls)
            .env("DESKACT_TEST_WINDOWS", windows)
            .env("DESKACT_TEST_NEW_SNAPSHOT", new_snapshot)
            .env("DESKACT_TEST_SAVE_SNAPSHOT", save_snapshot)
            .env("DESKACT_TEST_SAVED_SNAPSHOT", saved_snapshot)
            .env("DESKACT_TEST_MARKER", &self.marker)
            .env("DESKACT_TEST_FILENAME", &self.filename)
            .output()
            .expect("run developer harness")
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

fn make_executable(path: &std::path::Path) {
    let mut permissions = fs::metadata(path)
        .expect("read executable metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fixture executable");
}

#[test]
fn notepad_acceptance_creates_a_new_tab_in_the_unique_existing_window() {
    let fixture = Fixture::new();
    let existing = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"user-window","title":"neetcode.ps1 - Notepad","app_name":"notepad","accessible":true,"minimized":false,"visible":true,"pid":9876}]}"#;
    let new_tab = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"user-window","title":"Untitled - Notepad","app_name":"notepad","accessible":true,"minimized":false,"visible":true,"pid":9876}]}"#;
    let save_dialog = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"user-window","title":"Untitled - Notepad","app_name":"notepad","accessible":true,"minimized":false,"visible":true,"pid":9876},{"id":"save-dialog","title":"Save as","app_name":"Notepad.exe","accessible":true,"minimized":false,"visible":true,"pid":9876}]}"#;
    let saved_tab = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"user-window","title":"deskact-marker-0.txt - Notepad","app_name":"notepad","accessible":true,"minimized":false,"visible":true,"pid":9876}]}"#;
    let windows = format!("{existing}\n{new_tab}\n{save_dialog}\n{saved_tab}\n{existing}");
    let new_snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"notepad","window":{"id":"user-window","title":"Untitled - Notepad"},"snapshot_id":"snap-new","ref_count":1,"complete":true,"tree":{"role":"window","children":[{"role":"textfield","name":"Text Editor","ref_id":"@snap-new:e1","value":""}]}}}"#;
    let save_snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"Notepad.exe","window":{"id":"save-dialog","title":"Save as"},"snapshot_id":"snap-dialog","ref_count":2,"complete":true,"tree":{"role":"window","children":[{"role":"textfield","name":"File name:","ref_id":"@snap-dialog:e1"},{"role":"button","name":"Save","ref_id":"@snap-dialog:e2"}]}}}"#;
    let saved_snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"notepad","window":{"id":"user-window","title":"deskact-marker-0.txt - Notepad"},"snapshot_id":"snap-saved","ref_count":1,"complete":true,"tree":{"role":"window","children":[{"role":"textfield","name":"Text Editor","ref_id":"@snap-saved:e1","value":"DESKACT_NOTEPAD_ACCEPTANCE_MARKER"}]}}}"#;

    let output = fixture.run(&windows, new_snapshot, save_snapshot, saved_snapshot);
    let calls = fixture.calls();

    assert!(
        calls
            .iter()
            .any(|call| call == "press --headed --app notepad --window-id user-window ctrl+n"),
        "Ctrl+N must target the uniquely matched existing window: {calls:?}; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        calls
            .iter()
            .any(|call| call == "snapshot --app notepad --window-id user-window -i"),
        "the new tab must be freshly observed: {calls:?}"
    );
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}; calls: {calls:?}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        calls
            .iter()
            .any(|call| call == "get @snap-new:e1 --property value"),
        "new tab must be independently verified empty before mutation: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("set-value @snap-new:e1 ")),
        "editor marker was not entered: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("set-value @snap-dialog:e1 ")),
        "unique Save As path was not entered: {calls:?}"
    );
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.as_str() == "click @snap-dialog:e2")
            .count(),
        2,
        "an unverified Save click is observed and retried only from a fresh dialog snapshot: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call == "get @snap-saved:e1 --property value"),
        "editor content was not independently read back: {calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call == "press --headed --app notepad --window-id user-window ctrl+w"),
        "verified test tab was not closed: {calls:?}"
    );
    let path_line = calls
        .iter()
        .find(|call| call.starts_with("set-value @snap-dialog:e1 "))
        .unwrap();
    let saved_path = path_line
        .splitn(3, ' ')
        .nth(2)
        .expect("saved path argument");
    assert!(
        !std::path::Path::new(saved_path).exists(),
        "test-created saved file should be cleaned up"
    );
    assert!(
        !std::path::Path::new(saved_path).parent().unwrap().exists(),
        "unique per-run directory should be cleaned up"
    );
}

#[test]
fn notepad_acceptance_fails_closed_when_the_new_tab_identity_is_not_confirmed() {
    let fixture = Fixture::new();
    let existing = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"user-window","title":"neetcode.ps1 - Notepad","app_name":"notepad","accessible":true,"minimized":false,"visible":true,"pid":9876}]}"#;
    let unexpected = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"user-window","title":"notes.txt - Notepad","app_name":"notepad","accessible":true,"minimized":false,"visible":true,"pid":9876}]}"#;
    let windows = format!("{existing}\n{unexpected}");
    let output = fixture.run_count(20, &windows, "", "", "");
    let calls = fixture.calls();
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(
        stderr.contains("verified 0/1 attempted runs (20 requested)"),
        "{stderr}"
    );
    assert!(stderr.contains("transition was not confirmed"), "{stderr}");
    assert!(
        !calls
            .iter()
            .any(|call| call == "press --headed --app notepad --window-id user-window ctrl+w"),
        "unverified tab must never be closed: {calls:?}"
    );
}

#[test]
fn notepad_acceptance_refuses_ambiguous_preexisting_windows_without_actions() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"one","title":"one - Notepad","app_name":"notepad","accessible":true,"minimized":false,"pid":9876},{"id":"two","title":"two - Notepad","app_name":"notepad","accessible":true,"minimized":false,"pid":9876}]}"#;
    let output = fixture.run_count(20, windows, "", "", "");
    let calls = fixture.calls();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Expected exactly one existing Notepad window")
    );
    assert!(
        !calls.iter().any(|call| call.starts_with("press ")),
        "ambiguous windows must not receive actions: {calls:?}"
    );
}
