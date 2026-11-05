#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

struct FakeAgentDesktop {
    dir: PathBuf,
    executable: PathBuf,
    powershell: PathBuf,
    invocation_log: PathBuf,
}

impl FakeAgentDesktop {
    fn new(version_response: &str, apps_response: &str) -> Self {
        Self::with_catalog(
            version_response,
            apps_response,
            r#"{"entries":[],"skipped_start_menu_shortcuts":0}"#,
        )
    }

    fn with_catalog(version_response: &str, apps_response: &str, catalog_response: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("deskact-test-{}-{unique}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temporary test directory");
        let executable = dir.join("agent-desktop");
        let invocation_log = dir.join("invocations.log");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$1\" >> \"$DESKACT_TEST_INVOCATIONS\"\ncase \"$1\" in\n  version) printf '%s\\n' '{}' ;;\n  list-apps) printf '%s\\n' '{}'; exit \"${{DESKACT_TEST_APPS_EXIT_CODE:-0}}\" ;;\n  *) echo unexpected command >&2; exit 2 ;;\nesac\n",
                version_response,
                apps_response
            ),
        )
        .expect("write fake CLI");
        let mut permissions = fs::metadata(&executable)
            .expect("read fake CLI metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("make fake CLI executable");

        let powershell = dir.join("powershell");
        fs::write(
            &powershell,
            format!("#!/bin/sh\nprintf '%s\\n' '{}'\n", catalog_response),
        )
        .expect("write fake PowerShell");
        let mut permissions = fs::metadata(&powershell)
            .expect("read fake PowerShell metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&powershell, permissions).expect("make fake PowerShell executable");

        Self {
            dir,
            executable,
            powershell,
            invocation_log,
        }
    }

    fn path(&self) -> &Path {
        &self.executable
    }

    fn invocation_log(&self) -> &Path {
        &self.invocation_log
    }
}

impl Drop for FakeAgentDesktop {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn run_harness(fake: &FakeAgentDesktop, command: &str) -> Output {
    run_harness_with_apps_exit_code(fake, command, 0)
}

fn run_harness_with_apps_exit_code(
    fake: &FakeAgentDesktop,
    command: &str,
    exit_code: u8,
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .arg(command)
        .env("DESKACT_AGENT_DESKTOP", fake.path())
        .env("DESKACT_POWERSHELL", &fake.powershell)
        .env("DESKACT_TEST_INVOCATIONS", fake.invocation_log())
        .env("DESKACT_TEST_APPS_EXIT_CODE", exit_code.to_string())
        .output()
        .expect("run developer harness")
}

#[test]
fn notepad_acceptance_zero_runs_uses_the_usage_error_path() {
    let output = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .args(["notepad-acceptance", "--runs", "0"])
        .output()
        .expect("run developer harness");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr.contains("Usage: deskact-dev notepad-acceptance [--runs <count>]"),
        "stderr: {stderr}"
    );
}

#[test]
fn preflight_reports_the_supported_native_tool_and_contract() {
    let fake = FakeAgentDesktop::new(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[]}}"#,
    );

    let output = run_harness(&fake, "preflight");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("harness output is UTF-8");
    assert!(stdout.contains("agent-desktop 1.1.0"), "{stdout}");
    assert!(stdout.contains("JSON contract 2.6"), "{stdout}");
}

#[test]
fn preflight_rejects_an_incompatible_tool_version_with_setup_guidance() {
    let fake = FakeAgentDesktop::new(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.2.0"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[]}}"#,
    );

    let output = run_harness(&fake, "preflight");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("requires exactly 1.1.0"), "{stderr}");
    assert!(
        stderr.contains("npm install --global agent-desktop@1.1.0"),
        "{stderr}"
    );
}

#[test]
fn preflight_rejects_a_cli_reporting_a_non_windows_platform() {
    let fake = FakeAgentDesktop::new(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"linux","target":"x86_64","version":"1.1.0"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[]}}"#,
    );

    let output = run_harness(&fake, "preflight");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("requires Windows"), "{stderr}");
}

#[test]
fn preflight_rejects_an_unsupported_cli_architecture() {
    let fake = FakeAgentDesktop::new(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"arm64","version":"1.1.0"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[]}}"#,
    );

    let output = run_harness(&fake, "preflight");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("requires x86_64"), "{stderr}");
}

#[test]
fn preflight_checks_contract_before_deserializing_version_data() {
    let fake = FakeAgentDesktop::new(
        r#"{"version":"2.7","ok":true,"command":"version","data":{"incompatible":"version payload"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[]}}"#,
    );

    let output = run_harness(&fake, "preflight");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(
        stderr.contains("Unsupported agent-desktop JSON contract 2.7"),
        "{stderr}"
    );
    assert!(
        !stderr.contains("version response data was invalid"),
        "{stderr}"
    );
}

#[test]
fn preflight_reports_cli_error_before_decoding_failed_version_data() {
    let fake = FakeAgentDesktop::new(
        r#"{"version":"2.6","ok":false,"command":"version","data":{"unexpected":true},"error":{"message":"version unavailable"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[]}}"#,
    );

    let output = run_harness(&fake, "preflight");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("version unavailable"), "{stderr}");
    assert!(
        !stderr.contains("version response data was invalid"),
        "{stderr}"
    );
}

#[test]
fn preflight_rejects_an_incompatible_json_contract() {
    let fake = FakeAgentDesktop::new(
        r#"{"version":"2.7","ok":true,"command":"version","data":{"incompatible":"version payload"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[]}}"#,
    );

    let output = run_harness(&fake, "preflight");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(
        stderr.contains("Unsupported agent-desktop JSON contract 2.7"),
        "{stderr}"
    );
}

#[test]
fn inventory_preflight_failure_prevents_further_native_tool_operations() {
    let fake = FakeAgentDesktop::with_catalog(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.2.0"}}"#,
        "not-json",
        "not-json",
    );

    let output = run_harness(&fake, "inventory");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("requires exactly 1.1.0"), "{stderr}");
    assert!(!stderr.contains("invalid JSON"), "{stderr}");
    let calls = fs::read_to_string(fake.invocation_log()).expect("read required invocation log");
    assert_eq!(calls.lines().collect::<Vec<_>>(), ["version"]);
}

#[test]
fn inventory_lists_running_apps_in_a_stable_order() {
    let fake = FakeAgentDesktop::new(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[{"name":"Notepad.exe","pid":7736,"presentation":"foreground","process_instance":"opaque-1"},{"name":"CalculatorApp.exe","pid":1234,"presentation":"background","process_instance":"opaque-2"}]}}"#,
    );

    let output = run_harness(&fake, "inventory");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("harness output is UTF-8");
    assert!(stdout.contains("Currently running GUI apps: 2"), "{stdout}");
    assert!(
        stdout.find("CalculatorApp.exe").unwrap() < stdout.find("Notepad.exe").unwrap(),
        "{stdout}"
    );
}

#[test]
fn inventory_lists_the_discovered_start_menu_and_registered_app_catalog() {
    let fake = FakeAgentDesktop::with_catalog(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[]}}"#,
        r#"{"entries":[{"name":"Notepad","target":"C:\\Windows\\System32\\notepad.exe","arguments":"","working_directory":"","source":"start-menu"},{"name":"Calculator","target":"C:\\Windows\\System32\\calc.exe","arguments":"","working_directory":"","source":"app-paths"}],"skipped_start_menu_shortcuts":0}"#,
    );

    let output = run_harness(&fake, "inventory");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("harness output is UTF-8");
    assert!(stdout.contains("Discovered app entries: 2"), "{stdout}");
    assert!(stdout.contains("Calculator [app-paths]"), "{stdout}");
    assert!(stdout.contains("Notepad [start-menu]"), "{stdout}");
}

#[test]
fn inventory_preserves_discovered_targets_for_inspection() {
    let fake = FakeAgentDesktop::with_catalog(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[]}}"#,
        r#"{"entries":[{"name":"Notepad","target":"C:\\Windows\\System32\\notepad.exe","arguments":"","working_directory":"","source":"start-menu"},{"name":"Help","target":"C:\\Program Files\\Help\\manual.chm","arguments":"","working_directory":"","source":"start-menu"}],"skipped_start_menu_shortcuts":1}"#,
    );

    let output = run_harness(&fake, "inventory");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("harness output is UTF-8");
    assert!(stdout.contains("Discovered app entries: 2"), "{stdout}");
    assert!(stdout.contains("Help [start-menu]"), "{stdout}");
    assert!(stdout.contains("manual.chm"), "{stdout}");
    assert!(
        stdout.contains("Skipped Start Menu shortcuts without file targets: 1"),
        "{stdout}"
    );
}

#[test]
fn inventory_reports_windows_catalog_discovery_errors_without_claiming_success() {
    let fake = FakeAgentDesktop::with_catalog(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}"#,
        r#"{"version":"2.6","ok":true,"command":"list-apps","data":{"apps":[]}}"#,
        "not-json",
    );

    let output = run_harness(&fake, "inventory");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(
        stderr.contains("Windows app discovery returned invalid JSON"),
        "{stderr}"
    );
}

#[test]
fn inventory_uses_structured_cli_error_from_nonzero_exit() {
    let fake = FakeAgentDesktop::new(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}"#,
        r#"{"version":"2.6","ok":false,"command":"list-apps","error":{"message":"access denied"}}"#,
    );

    let output = run_harness_with_apps_exit_code(&fake, "inventory", 17);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("access denied"), "{stderr}");
    assert!(!stderr.contains("exited with status"), "{stderr}");
}

#[test]
fn inventory_reports_invalid_cli_output_without_claiming_success() {
    let fake = FakeAgentDesktop::new(
        r#"{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}"#,
        "not-json",
    );

    let output = run_harness(&fake, "inventory");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(stderr.contains("returned invalid JSON"), "{stderr}");
}

#[test]
fn preflight_reports_actionable_guidance_when_the_cli_is_missing() {
    let missing =
        std::env::temp_dir().join(format!("missing-agent-desktop-{}", std::process::id()));
    let output = Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
        .arg("preflight")
        .env("DESKACT_AGENT_DESKTOP", missing)
        .output()
        .expect("run developer harness");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success());
    assert!(
        stderr.contains("npm install --global agent-desktop@1.1.0"),
        "{stderr}"
    );
    assert!(stderr.contains("DESKACT_AGENT_DESKTOP"), "{stderr}");
    assert!(stderr.contains("agent-desktop-win32-x64.exe"), "{stderr}");
}
