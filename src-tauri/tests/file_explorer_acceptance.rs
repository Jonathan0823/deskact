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
        fs::write(&cli, r##"#!/bin/sh
printf '%s\n' "$*" >> "$DESKACT_TEST_CALLS"
case "$1" in
 version) echo '{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}' ;;
 list-windows) if [ "$(grep -c '^press .* enter' "$DESKACT_TEST_CALLS")" -ge 2 ] && grep -q '^click ' "$DESKACT_TEST_CALLS"; then path=$(grep '^set-value ' "$DESKACT_TEST_CALLS" | tail -n 1 | cut -d' ' -f3-); suffix=${path##*/}; suffix=${suffix#deskact-explorer-}; file="deskact-fixture-${suffix}.txt"; printf '%s\n' "$DESKACT_TEST_OPEN_WINDOWS" | sed "s/__FILE__/$file/g"; else echo "$DESKACT_TEST_WINDOWS"; fi ;;
 snapshot) if printf '%s' "$*" | grep -q -- '--app notepad.exe'; then path=$(grep '^set-value ' "$DESKACT_TEST_CALLS" | tail -n 1 | cut -d' ' -f3-); suffix=${path##*/}; suffix=${suffix#deskact-explorer-}; marker="DESKACT_FILE_EXPLORER_FIXTURE_${suffix}"; printf '%s\n' "$DESKACT_TEST_OPEN_SNAPSHOT" | sed "s/__MARKER__/$marker/g"; else n=$(grep -c '^snapshot ' "$DESKACT_TEST_CALLS"); path=$(grep '^set-value ' "$DESKACT_TEST_CALLS" | tail -n 1 | cut -d' ' -f3-); suffix=${path##*/}; suffix=${suffix#deskact-explorer-}; file="deskact-fixture-${suffix}.txt"; state=; [ "$n" -ge 5 ] && state='"selected"'; printf '{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"explorer.exe","window":{"id":"explorer","title":"File Explorer"},"snapshot_id":"fresh-%s","ref_count":3,"complete":true,"tree":{"role":"window","children":[{"role":"textfield","name":"Address Bar","states":["focused"],"ref_id":"@fresh:e1","value":"%s"},{"role":"option","name":"%s","states":[%s],"ref_id":"@fresh:e2"}]}}}\n' "$n" "$([ "$n" -eq 1 ] && echo '' || echo "$path")" "$file" "$state"; fi ;;
 focus-window) echo '{"version":"2.6","ok":true,"command":"focus-window","data":{"focused":{"id":"explorer","is_focused":true}}}' ;;
 click|set-value|press) echo "{\"version\":\"2.6\",\"ok\":true,\"command\":\"$1\",\"data\":{\"disposition\":\"delivered_unverified\"}}" ;;
 *) echo "unexpected command: $1" >&2; exit 2 ;;
esac
"##).unwrap();
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
    let opened_windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"explorer","title":"File Explorer","app_name":"explorer","accessible":true,"minimized":false,"visible":true,"pid":42},{"id":"opened-file","title":"__FILE__ - Notepad","app_name":"notepad.exe","accessible":true,"minimized":false,"visible":true,"pid":43}]}"#;
    let open_snapshot = r#"{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"notepad.exe","window":{"id":"opened-file","title":"__FILE__ - Notepad"},"snapshot_id":"opened","ref_count":1,"complete":true,"tree":{"role":"window","children":[{"role":"textfield","name":"Text Editor","ref_id":"@opened:e1","value":"__MARKER__"}]}}}"#;
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
        calls.contains("set-value @fresh:e1 "),
        "folder path must be entered through UI: {calls}"
    );
    assert!(
        calls.contains("click @fresh:e2"),
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
