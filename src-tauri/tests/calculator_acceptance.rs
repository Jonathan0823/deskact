#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

const CALCULATOR_WINDOW: &str = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"calculator-window","title":"Calculator","app_name":"CalculatorApp.exe","accessible":true,"minimized":false,"visible":true,"pid":9876}]}"#;

struct Fixture {
    dir: PathBuf,
    cli: PathBuf,
    powershell: PathBuf,
    calls: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "deskact-calculator-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create fixture directory");
        let cli = dir.join("agent-desktop");
        let powershell = dir.join("powershell");
        let calls = dir.join("calls.log");
        let state = dir.join("calculator-state");
        fs::write(&state, "clean").expect("write initial calculator state");
        fs::write(
            &cli,
            r##"#!/bin/sh
printf '%s\n' "$*" >> "$DESKACT_TEST_CALLS"
latest_snapshot() { grep -c '^snapshot ' "$DESKACT_TEST_CALLS"; }
error_stale_ref() { printf '%s\n' '{"version":"2.6","ok":false,"command":"'"$1"'","error":{"code":"STALE_REF","message":"reference is not from the latest snapshot"}}'; exit 1; }
case "$1" in
 version) echo '{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}' ;;
 list-windows) if grep -q '^powershell-launch ' "$DESKACT_TEST_CALLS"; then echo "$DESKACT_TEST_WINDOWS"; else echo "$DESKACT_TEST_INITIAL_WINDOWS"; fi ;;
 snapshot)
  n=$(latest_snapshot); state=$(cat "$DESKACT_TEST_STATE"); display=0; expression='Expression is '
  case "$state" in seven) display=7; expression='Expression is 7' ;; multiply) display=7; expression='Expression is 7 ×' ;; six) display=6; expression='Expression is 7 × 6' ;; done) display=42; expression='Expression is 7 × 6 ='; [ "$DESKACT_TEST_WRONG_RESULT" = true ] && display=43 ;; esac
  result_node="{\"role\":\"statictext\",\"name\":\"Display is $display\",\"native_id\":{\"kind\":\"automation_id\",\"value\":\"CalculatorResults\"}}"; [ "$state" = done ] && [ "$DESKACT_TEST_MISSING_RESULT" = true ] && result_node='{"role":"statictext","name":"Display unavailable"}';
  expression_node="{\"role\":\"statictext\",\"name\":\"$expression\",\"native_id\":{\"kind\":\"automation_id\",\"value\":\"CalculatorExpression\"}}"; [ "$DESKACT_TEST_MISSING_EXPRESSION" = true ] && expression_node='{"role":"statictext","name":"Expression unavailable"}';
  clear_state=; [ "$state" = clean ] && [ "$DESKACT_TEST_MISSING_EXPRESSION" != true ] && clear_state='"disabled"';
  printf '{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"CalculatorApp.exe","window":{"id":"calculator-window","title":"Calculator"},"snapshot_id":"snapshot-%s","ref_count":5,"complete":true,"tree":{"role":"window","children":[%s,%s,{"role":"button","name":"Clear","states":[%s],"ref_id":"@snapshot-%s:e1"},{"role":"button","name":"Seven","ref_id":"@snapshot-%s:e2"},{"role":"button","name":"Multiply by","ref_id":"@snapshot-%s:e3"},{"role":"button","name":"Six","ref_id":"@snapshot-%s:e4"},{"role":"button","name":"Equals","ref_id":"@snapshot-%s:e5"}]}}}\n' "$n" "$result_node" "$expression_node" "$clear_state" "$n" "$n" "$n" "$n" "$n" ;;
 click)
  n=$(latest_snapshot); expected_prefix="@snapshot-${n}:e"; case "$2" in "$expected_prefix"*) ;; *) error_stale_ref click ;; esac
  case "${2##*:e}" in
   1) if [ "$(cat "$DESKACT_TEST_STATE")" = clean ] && [ "$DESKACT_TEST_MISSING_EXPRESSION" != true ]; then error_stale_ref click; fi; echo clean > "$DESKACT_TEST_STATE" ;;
   2) [ "$(cat "$DESKACT_TEST_STATE")" = clean ] || error_stale_ref click; echo seven > "$DESKACT_TEST_STATE" ;;
   3) [ "$(cat "$DESKACT_TEST_STATE")" = seven ] || error_stale_ref click; echo multiply > "$DESKACT_TEST_STATE" ;;
   4) [ "$(cat "$DESKACT_TEST_STATE")" = multiply ] || error_stale_ref click; echo six > "$DESKACT_TEST_STATE" ;;
   5) [ "$(cat "$DESKACT_TEST_STATE")" = six ] || error_stale_ref click; echo done > "$DESKACT_TEST_STATE" ;;
   *) error_stale_ref click ;;
  esac
  echo '{"version":"2.6","ok":true,"command":"click","data":{"disposition":{"delivery":"delivered_unverified","retry":"unsafe"}}}' ;;
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
  *Win32_Process*) printf '%s\n' 'C:\Program Files\WindowsApps\Microsoft.WindowsCalculator_1.0.0.0_x64__8wekyb3d8bbwe\CalculatorApp.exe' ;;
  *Start-Process*) printf 'powershell-launch %s\n' "$*" >> "$DESKACT_TEST_CALLS" ;;
  *) echo 'unexpected PowerShell command' >&2; exit 2 ;;
esac
"##,
        )
        .expect("write fake PowerShell");
        make_executable(&powershell);
        Self {
            dir,
            cli,
            powershell,
            calls,
            state,
        }
    }

    fn run(&self) -> std::process::Output {
        self.run_with_state(
            r#"{"version":"2.6","ok":true,"command":"list-windows","data":[]}"#,
            "clean",
            false,
            false,
            false,
        )
    }

    fn run_with_state(
        &self,
        initial_windows: &str,
        initial_state: &str,
        wrong_result: bool,
        missing_result: bool,
        missing_expression: bool,
    ) -> std::process::Output {
        fs::write(&self.state, initial_state).expect("set initial calculator state");
        Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .arg("calculator-acceptance")
            .env("DESKACT_AGENT_DESKTOP", &self.cli)
            .env("DESKACT_POWERSHELL", &self.powershell)
            .env("DESKACT_TEST_CALLS", &self.calls)
            .env("DESKACT_TEST_STATE", &self.state)
            .env("DESKACT_TEST_WINDOWS", CALCULATOR_WINDOW)
            .env("DESKACT_TEST_INITIAL_WINDOWS", initial_windows)
            .env("DESKACT_TEST_WRONG_RESULT", wrong_result.to_string())
            .env("DESKACT_TEST_MISSING_RESULT", missing_result.to_string())
            .env(
                "DESKACT_TEST_MISSING_EXPRESSION",
                missing_expression.to_string(),
            )
            .output()
            .expect("run developer harness")
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

fn make_executable(path: &std::path::Path) {
    let mut permissions = fs::metadata(path)
        .expect("read executable metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fixture executable");
}

/// Run a fresh Calculator, use current refs for each keypad action, and verify the displayed result.
#[test]
fn calculator_acceptance_launches_and_verifies_seven_times_six() {
    let fixture = Fixture::new();
    let output = fixture.run();
    let calls = fixture.calls();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        output.status.success(),
        "stdout: {stdout}\nstderr: {stderr}\ncalls: {calls}"
    );
    assert!(stdout.contains("verified 7 × 6 = 42"), "{stdout}");
    assert!(
        calls.contains("shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
        "Calculator's packaged app ID was not launched: {calls}"
    );
    assert!(
        calls.contains("click @snapshot-1:e2"),
        "fresh Seven ref missing: {calls}"
    );
    assert!(
        calls.contains("click @snapshot-2:e3"),
        "fresh Multiply ref missing: {calls}"
    );
    assert!(
        calls.contains("click @snapshot-3:e4"),
        "fresh Six ref missing: {calls}"
    );
    assert!(
        calls.contains("click @snapshot-4:e5"),
        "fresh Equals ref missing: {calls}"
    );
    assert!(
        calls.contains("snapshot --app CalculatorApp.exe --window-id calculator-window -i"),
        "final result was not freshly observed: {calls}"
    );
}

/// Refuse multiple Calculator windows without launching or mutating either one.
#[test]
fn calculator_acceptance_fails_closed_for_ambiguous_windows() {
    let fixture = Fixture::new();
    let windows = r#"{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"one","title":"Calculator","app_name":"CalculatorApp.exe","accessible":true,"minimized":false,"visible":true,"pid":9876},{"id":"two","title":"Calculator","app_name":"CalculatorApp.exe","accessible":true,"minimized":false,"visible":true,"pid":9877}]}"#;
    let output = fixture.run_with_state(windows, "clean", false, false, false);
    let calls = fixture.calls();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("found 2"));
    assert!(
        !calls.contains("launch ") && !calls.contains("snapshot ") && !calls.contains("click "),
        "ambiguous Calculator windows were acted on: {calls}"
    );
}

/// Reset only the Calculator display before a repeat run when a prior result is present.
#[test]
fn calculator_acceptance_clears_prior_display_before_entering_the_fixture_calculation() {
    let fixture = Fixture::new();
    let output = fixture.run_with_state(CALCULATOR_WINDOW, "done", false, false, false);
    let calls = fixture.calls();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stdout: {stdout}; stderr: {}; calls: {calls}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        calls.contains("click @snapshot-1:e1"),
        "prior result was not cleared from a fresh control ref: {calls}"
    );
    assert!(
        calls.contains("click @snapshot-2:e2"),
        "new calculation did not use the post-clear snapshot: {calls}"
    );
    assert!(stdout.contains("verified 7 × 6 = 42"), "{stdout}");
}

/// Do not claim success if fresh Calculator UI evidence disagrees with the expected result.
#[test]
fn calculator_acceptance_fails_when_fresh_display_is_not_42() {
    let fixture = Fixture::new();
    let output = fixture.run_with_state(
        r#"{"version":"2.6","ok":true,"command":"list-windows","data":[]}"#,
        "clean",
        true,
        false,
        false,
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not the expected result `42`"));
}

/// Reset a clean-looking display when Calculator omits the optional expression node.
#[test]
fn calculator_acceptance_clears_when_expression_evidence_is_not_exposed() {
    let fixture = Fixture::new();
    let output = fixture.run_with_state(
        r#"{"version":"2.6","ok":true,"command":"list-windows","data":[]}"#,
        "clean",
        false,
        false,
        true,
    );
    let calls = fixture.calls();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stdout: {stdout}; stderr: {}; calls: {calls}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        calls.contains("click @snapshot-1:e1"),
        "missing expression evidence should trigger a fresh Clear action: {calls}"
    );
    assert!(
        calls.contains("click @snapshot-2:e2"),
        "key input should use the post-Clear snapshot: {calls}"
    );
}

/// Do not claim success if the final snapshot cannot expose the Calculator display.
#[test]
fn calculator_acceptance_fails_when_final_display_evidence_is_missing() {
    let fixture = Fixture::new();
    let output = fixture.run_with_state(
        r#"{"version":"2.6","ok":true,"command":"list-windows","data":[]}"#,
        "clean",
        false,
        true,
        false,
    );
    let calls = fixture.calls();
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("final snapshot did not expose its display")
    );
    assert!(
        calls.contains("snapshot --app CalculatorApp.exe --window-id calculator-window -i"),
        "final evidence was not freshly inspected: {calls}"
    );
}
