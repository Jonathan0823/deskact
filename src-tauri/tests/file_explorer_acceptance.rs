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
}

impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "deskact-explorer-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let cli = dir.join("agent-desktop");
        let powershell = dir.join("powershell");
        let calls = dir.join("calls.log");
        fs::write(
            &cli,
            r##"#!/bin/sh
printf '%s\n' "$*" >> "$DESKACT_TEST_CALLS"
latest_explorer_snapshot() { grep -c '^snapshot --app explorer ' "$DESKACT_TEST_CALLS"; }
fixture_suffix() { path=$(grep '^set-value ' "$DESKACT_TEST_CALLS" | tail -n 1 | cut -d' ' -f3-); printf '%s' "${path##*deskact-explorer-}"; }
error_stale_ref() { printf '%s\n' '{"version":"2.6","ok":false,"command":"'"$1"'","error":{"code":"STALE_REF","message":"reference is not from the latest snapshot"}}'; exit 1; }
case "$1" in
 version) echo '{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}' ;;
 list-windows)
  if [ "$(grep -c '^press --headed --app explorer --window-id explorer enter$' "$DESKACT_TEST_CALLS")" -ge 2 ] && grep -q '^click ' "$DESKACT_TEST_CALLS"; then
   suffix=$(fixture_suffix); file="deskact-fixture-${suffix}.txt"; printf '%s\n' "$DESKACT_TEST_OPEN_WINDOWS" | sed "s/__FILE__/$file/g"
  else echo "$DESKACT_TEST_WINDOWS"; fi ;;
 snapshot)
  if [ "$2" = "--app" ] && [ "$3" = "notepad.exe" ]; then
   n=$(grep -c '^snapshot --app notepad.exe ' "$DESKACT_TEST_CALLS"); suffix=$(fixture_suffix); marker="DESKACT_FILE_EXPLORER_FIXTURE_${suffix}"; printf '%s\n' "$DESKACT_TEST_OPEN_SNAPSHOT" | sed -e "s/__MARKER__/$marker/g" -e "s/__SNAPSHOT__/viewer-${n}/g" -e "s/__REF__/@viewer-${n}:e1/g"
  else
   n=$(latest_explorer_snapshot); path=$(grep '^set-value ' "$DESKACT_TEST_CALLS" | tail -n 1 | cut -d' ' -f3-); escaped_path=$(printf '%s' "$path" | sed 's/\\/\\\\/g'); suffix=${path##*deskact-explorer-}; file="deskact-fixture-${suffix}.txt"; state=; [ "$n" -ge 5 ] && state='"selected"';
   response=$(printf '{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"explorer.exe","window":{"id":"explorer","title":"File Explorer"},"snapshot_id":"fresh-%s","ref_count":3,"complete":true,"tree":{"role":"window","children":[{"role":"textfield","name":"Address Bar","states":["focused"],"ref_id":"@fresh-%s:e1","value":"%s"},{"role":"option","name":"%s","states":[%s],"ref_id":"@fresh-%s:e2"}]}}}\n' "$n" "$n" "$escaped_path" "$file" "$state" "$n"); printf '%s\n' "$response"
  fi ;;
 focus-window) echo '{"version":"2.6","ok":true,"command":"focus-window","data":{"focused":{"id":"explorer","is_focused":true}}}' ;;
 set-value) n=$(latest_explorer_snapshot); [ "$2" = "@fresh-${n}:e1" ] || error_stale_ref set-value; echo '{"version":"2.6","ok":true,"command":"set-value","data":{"disposition":"delivered_unverified"}}' ;;
 click) n=$(latest_explorer_snapshot); [ "$2" = "@fresh-${n}:e2" ] || error_stale_ref click; echo '{"version":"2.6","ok":true,"command":"click","data":{"disposition":"delivered_unverified"}}' ;;
 press) echo '{"version":"2.6","ok":true,"command":"press","data":{"disposition":"delivered_unverified"}}' ;;
 *) echo "unexpected command: $1" >&2; exit 2 ;;
esac
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&cli).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&cli, permissions).unwrap();
        fs::write(
            &powershell,
            r##"#!/bin/sh
printf '%s\n' 'C:\Windows\explorer.exe'
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&powershell).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&powershell, permissions).unwrap();
        Self {
            dir,
            cli,
            powershell,
            calls,
        }
    }
    fn run(&self, windows: &str, open_windows: &str, open_snapshot: &str) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .args(["file-explorer-acceptance"])
            .env("DESKACT_AGENT_DESKTOP", &self.cli)
            .env("DESKACT_POWERSHELL", &self.powershell)
            .env("DESKACT_TEST_CALLS", &self.calls)
            .env("DESKACT_TEST_WINDOWS", windows)
            .env("DESKACT_TEST_OPEN_WINDOWS", open_windows)
            .env("DESKACT_TEST_OPEN_SNAPSHOT", open_snapshot)
            .env("WINDIR", "C:\\Windows")
            .output()
            .unwrap()
    }
    fn calls(&self) -> String {
        fs::read_to_string(&self.calls).unwrap_or_default()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn file_explorer_acceptance_navigates_and_opens_through_fresh_accessible_controls() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"explorer","title":"File Explorer","app_name":"explorer","accessible":true,"minimized":false,"visible":true,"pid":42}]}"#;
    let opened_windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"explorer","title":"File Explorer","app_name":"explorer","accessible":true,"minimized":false,"visible":true,"pid":42},{"id":"opened-file","title":"Notepad","app_name":"notepad.exe","accessible":true,"minimized":false,"visible":true,"pid":43,"is_focused":true}]}"#;
    let open_snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"notepad.exe","window":{"id":"opened-file","title":"Notepad"},"snapshot_id":"__SNAPSHOT__","ref_count":1,"complete":true,"tree":{"role":"window","children":[{"role":"textfield","name":"Text Editor","ref_id":"__REF__","value":"__MARKER__"}]}}}"#;
    let output = fixture.run(windows, opened_windows, open_snapshot);
    let calls = fixture.calls();
    assert!(
        output.status.success(),
        "stdout: {}; stderr: {}; calls: {calls}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        calls.contains("snapshot --app explorer --window-id explorer -i"),
        "{calls}"
    );
    assert!(
        calls.contains("focus-window --app explorer --window-id explorer"),
        "the selected Explorer window must be focused explicitly: {calls}"
    );
    assert!(
        calls.contains("press --headed --app explorer --window-id explorer ctrl+l"),
        "address bar must be focused through Explorer: {calls}"
    );
    assert!(
        calls.contains("set-value @fresh-1:e1 "),
        "folder path must be entered through UI: {calls}"
    );
    assert!(
        calls.contains("click @fresh-4:e2"),
        "fixture must be opened through an accessible control: {calls}"
    );
    assert!(
        calls.contains("press --headed --app explorer --window-id explorer enter"),
        "fixture opening must be confirmed through Explorer: {calls}"
    );
    assert!(
        calls.contains("snapshot --app notepad.exe --window-id opened-file -i"),
        "the opened file window must be freshly observed: {calls}"
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("verified"));
}

/// Ensure the fake CLI enforces the same snapshot-scoped reference contract as the real tool.
#[test]
fn file_explorer_fake_cli_rejects_a_reference_from_an_older_snapshot() {
    let fixture = Fixture::new();
    fs::write(
        &fixture.calls,
        "snapshot --app explorer --window-id explorer -i\nsnapshot --app explorer --window-id explorer -i\n",
    )
    .unwrap();
    let output = Command::new(&fixture.cli)
        .args(["click", "@fresh-1:e2"])
        .env("DESKACT_TEST_CALLS", &fixture.calls)
        .output()
        .unwrap();
    let response = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output.status.success());
    assert!(response.contains("STALE_REF"), "{response}");
}

/// Reject an otherwise successful viewer snapshot that lacks the exact fixture marker.
#[test]
fn file_explorer_acceptance_fails_when_opened_viewer_does_not_show_fixture_marker() {
    let fixture = Fixture::new();
    let explorer = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"explorer","title":"File Explorer","app_name":"explorer","accessible":true,"minimized":false,"visible":true,"pid":42}]}"#;
    let opened = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"opened-file","title":"__FILE__ - Notepad","app_name":"notepad.exe","accessible":true,"minimized":false,"visible":true,"pid":43,"is_focused":true}]}"#;
    let snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"notepad.exe","window":{"id":"opened-file","title":"__FILE__ - Notepad"},"snapshot_id":"__SNAPSHOT__","ref_count":1,"complete":true,"tree":{"role":"window","children":[{"role":"textfield","name":"Text Editor","ref_id":"__REF__","value":"unexpected contents"}]}}}"#;
    let output = fixture.run(explorer, opened, snapshot);
    let calls = fixture.calls();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("readiness deadline"));
    assert!(
        calls.contains("snapshot --app notepad.exe --window-id opened-file -i"),
        "the opened window was not freshly inspected: {calls}"
    );
}

/// Refuse to inspect a viewer when focused non-Explorer candidates are ambiguous.
#[test]
fn file_explorer_acceptance_fails_when_focused_viewer_candidates_are_ambiguous() {
    let fixture = Fixture::new();
    let explorer = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"explorer","title":"File Explorer","app_name":"explorer","accessible":true,"minimized":false,"visible":true,"pid":42}]}"#;
    let opened = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"one","title":"Notepad","app_name":"notepad.exe","accessible":true,"minimized":false,"visible":true,"pid":43,"is_focused":true},{"id":"two","title":"Text Editor","app_name":"editor.exe","accessible":true,"minimized":false,"visible":true,"pid":44,"is_focused":true}]}"#;
    let output = fixture.run(explorer, opened, "{}");
    let calls = fixture.calls();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("focused viewer"));
    assert!(
        !calls.contains("snapshot --app notepad.exe")
            && !calls.contains("snapshot --app editor.exe"),
        "ambiguous viewer candidates must not be inspected: {calls}"
    );
}

#[test]
fn file_explorer_acceptance_fails_closed_when_file_open_evidence_is_ambiguous() {
    let fixture = Fixture::new();
    let explorer = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"explorer","title":"File Explorer","app_name":"explorer","accessible":true,"minimized":false,"visible":true,"pid":42}]}"#;
    let opened = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"one","title":"__FILE__ - Notepad","app_name":"notepad.exe","accessible":true,"minimized":false,"visible":true,"pid":43},{"id":"two","title":"__FILE__ - Notepad","app_name":"notepad.exe","accessible":true,"minimized":false,"visible":true,"pid":44}]}"#;
    let output = fixture.run(explorer, opened, "{}");
    let calls = fixture.calls();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("opened-file evidence is ambiguous"));
    assert!(
        !calls.contains("snapshot --app notepad.exe"),
        "ambiguous opened-file windows must not be inspected or selected: {calls}"
    );
}

#[test]
fn file_explorer_acceptance_fails_closed_on_ambiguous_windows() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"one","title":"File Explorer","app_name":"explorer","accessible":true,"minimized":false,"visible":true,"pid":42},{"id":"two","title":"File Explorer","app_name":"explorer","accessible":true,"minimized":false,"visible":true,"pid":43}]}"#;
    let output = fixture.run(windows, "{}", "{}");
    let calls = fixture.calls();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Expected exactly one accessible Explorer window")
    );
    assert!(
        !calls.contains("snapshot ") && !calls.contains("set-value ") && !calls.contains("click "),
        "ambiguous window received UI actions: {calls}"
    );
}
