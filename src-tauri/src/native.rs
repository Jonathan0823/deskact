//! Native session: the verified `agent-desktop` CLI adapter, bounded subprocess execution, and deadline polling.

use std::{
    env,
    ffi::{OsStr, OsString},
    io::{self, Read},
    process::{Child, Command, ExitStatus, Output, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::Deserialize;

use crate::{
    APP_READINESS_TIMEOUT, AppSelectionInput, DeskActError, DiscoveredApp, NativeInventory,
    NativeTargetResolution, NativeWindow, PreflightStatus, ResolvedNativeTarget, RunningApp,
    SUPPORTED_AGENT_DESKTOP_VERSION, SUPPORTED_JSON_CONTRACT_VERSION, load_app_catalog,
    prepare_app_selection, process_image_path, windows_catalog,
};

const INSTALL_GUIDANCE: &str = "Install the required version from Windows PowerShell with `npm install --global agent-desktop@1.1.0`; DeskAct never installs or updates automation tools.";
/// Hard deadline applied to each native subprocess invocation.
pub(crate) const NATIVE_PROCESS_TIMEOUT: Duration = Duration::from_secs(30);
/// Pause between attempts of a deadline poll.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Failure to start a subprocess or complete it before the native-operation deadline.
#[derive(Debug)]
pub(crate) enum CommandRunError {
    Io(io::Error),
    TimedOut,
}

/// Retry `attempt` every 100 ms until it yields a value or `deadline` passes.
///
/// `attempt` always runs at least once and receives the time left (never zero); it returns `Ok(None)` to poll again.
/// When the deadline is reached without a value, `on_timeout` decides the outcome; it
/// may still produce a value, so callers can fall back to the last observed state.
pub(crate) fn poll_until_deadline<T>(
    deadline: Instant,
    mut attempt: impl FnMut(Duration) -> Result<Option<T>, DeskActError>,
    on_timeout: impl FnOnce() -> Result<T, DeskActError>,
) -> Result<T, DeskActError> {
    loop {
        // Always attempt at least once, even when the deadline passed before entry.
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .max(Duration::from_millis(1));
        if let Some(value) = attempt(remaining)? {
            return Ok(value);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return on_timeout();
        }
        thread::sleep(POLL_INTERVAL.min(remaining));
    }
}

/// One PowerShell invocation bounded by the native-process deadline.
pub(crate) struct PowerShellRun<'a> {
    /// Script passed to `-Command`.
    pub script: &'a str,
    /// Whether to pass `-NonInteractive`.
    pub non_interactive: bool,
    /// Capture stdout/stderr; otherwise output is discarded so inherited handles cannot block the wait.
    pub capture_output: bool,
    /// Extra environment variables for the child.
    pub envs: &'a [(&'a str, &'a str)],
    /// Completes "Could not start PowerShell to ...: {error}".
    pub action: &'a str,
    /// Complete message used when the deadline is exceeded.
    pub timeout_message: String,
}

/// Run PowerShell (`DESKACT_POWERSHELL` or `powershell.exe`) and map spawn failures and timeouts to diagnostics.
pub(crate) fn run_powershell(run: PowerShellRun<'_>) -> Result<Output, DeskActError> {
    let powershell =
        env::var_os("DESKACT_POWERSHELL").unwrap_or_else(|| OsString::from("powershell.exe"));
    let mut command = Command::new(powershell);
    command.arg("-NoProfile");
    if run.non_interactive {
        command.arg("-NonInteractive");
    }
    command
        .args(["-Command", run.script])
        .envs(run.envs.iter().copied());
    let result = if run.capture_output {
        run_command_with_timeout(&mut command, NATIVE_PROCESS_TIMEOUT)
    } else {
        run_command_status_with_timeout(&mut command, NATIVE_PROCESS_TIMEOUT).map(|status| Output {
            status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    };
    match result {
        Ok(output) => Ok(output),
        Err(CommandRunError::TimedOut) => Err(DeskActError(run.timeout_message)),
        Err(CommandRunError::Io(error)) => Err(DeskActError(format!(
            "Could not start PowerShell to {}: {error}",
            run.action
        ))),
    }
}

/// A native CLI capability created only after the installed version and JSON contract pass preflight.
/// Future native operations should be methods on this verified connection.
pub struct VerifiedNativeCli {
    executable: OsString,
    preflight: PreflightStatus,
}

impl VerifiedNativeCli {
    /// Connect to the configured executable after validating its version and platform.
    pub fn connect() -> Result<Self, DeskActError> {
        let executable = configured_executable();
        let output = run_agent_desktop(&executable, "version")?;
        let envelope: Envelope<serde_json::Value> = parse_envelope(&output, "version")?;

        if envelope.version != SUPPORTED_JSON_CONTRACT_VERSION {
            return Err(DeskActError(format!(
                "Unsupported agent-desktop JSON contract {} (DeskAct requires {}). {INSTALL_GUIDANCE}",
                envelope.version, SUPPORTED_JSON_CONTRACT_VERSION
            )));
        }
        if !envelope.ok {
            return Err(cli_error("version", envelope.error));
        }
        let value = envelope.data.ok_or_else(|| {
            DeskActError("agent-desktop version response did not include data".into())
        })?;
        let data: VersionData = serde_json::from_value(value).map_err(|error| {
            DeskActError(format!(
                "agent-desktop version response data was invalid: {error}"
            ))
        })?;
        if data.version != SUPPORTED_AGENT_DESKTOP_VERSION {
            return Err(DeskActError(format!(
                "Found agent-desktop {}, but DeskAct requires exactly {}. {INSTALL_GUIDANCE}",
                data.version, SUPPORTED_AGENT_DESKTOP_VERSION
            )));
        }
        if !data.os.eq_ignore_ascii_case("windows") {
            return Err(DeskActError(format!(
                "agent-desktop reports platform `{}`; DeskAct native control requires Windows x86_64.",
                data.os
            )));
        }
        if data.target != "x86_64" {
            return Err(DeskActError(format!(
                "agent-desktop reports target `{}`; DeskAct native control requires x86_64.",
                data.target
            )));
        }

        Ok(Self {
            executable,
            preflight: PreflightStatus {
                tool_version: data.version,
                contract_version: envelope.version,
            },
        })
    }

    /// Return the compatibility information verified when this connection was created.
    pub fn preflight_status(&self) -> &PreflightStatus {
        &self.preflight
    }

    /// List fresh windows matching an app label using the verified CLI connection.
    pub(crate) fn windows_for_app(&self, app: &str) -> Result<Vec<NativeWindow>, DeskActError> {
        let output = self.invoke_args(&["list-windows", "--app", app], "list-windows")?;
        let envelope: Envelope<Vec<NativeWindow>> = parse_envelope(&output, "list-windows")?;
        if envelope.version != SUPPORTED_JSON_CONTRACT_VERSION {
            return Err(DeskActError(format!(
                "Unsupported agent-desktop JSON contract {} in window inventory (DeskAct requires {}).",
                envelope.version, SUPPORTED_JSON_CONTRACT_VERSION
            )));
        }
        if !envelope.ok {
            return Err(cli_error("list-windows", envelope.error));
        }
        envelope.data.ok_or_else(|| {
            DeskActError("agent-desktop window inventory did not include data".into())
        })
    }

    /// List every top-level window while respecting a caller's remaining deadline.
    pub(crate) fn all_windows_with_timeout(
        &self,
        timeout: Duration,
    ) -> Result<Vec<NativeWindow>, DeskActError> {
        let output = self.invoke_args_with_timeout(&["list-windows"], "list-windows", timeout)?;
        let envelope: Envelope<Vec<NativeWindow>> = parse_envelope(&output, "list-windows")?;
        if envelope.version != SUPPORTED_JSON_CONTRACT_VERSION {
            return Err(DeskActError(format!(
                "Unsupported agent-desktop JSON contract {} in window inventory (DeskAct requires {}).",
                envelope.version, SUPPORTED_JSON_CONTRACT_VERSION
            )));
        }
        if !envelope.ok {
            return Err(cli_error("list-windows", envelope.error));
        }
        envelope.data.ok_or_else(|| {
            DeskActError("agent-desktop window inventory did not include data".into())
        })
    }

    /// List running GUI applications using this preflight-verified CLI connection.
    pub fn running_apps(&self) -> Result<Vec<RunningApp>, DeskActError> {
        let output = self.invoke("list-apps")?;
        let envelope: Envelope<AppInventory> = parse_envelope(&output, "list-apps")?;

        if envelope.version != SUPPORTED_JSON_CONTRACT_VERSION {
            return Err(DeskActError(format!(
                "Unsupported agent-desktop JSON contract {} in app inventory (DeskAct requires {}).",
                envelope.version, SUPPORTED_JSON_CONTRACT_VERSION
            )));
        }
        if !envelope.ok {
            return Err(cli_error("list-apps", envelope.error));
        }

        let mut apps = envelope
            .data
            .ok_or_else(|| DeskActError("agent-desktop app inventory did not include data".into()))?
            .apps;
        apps.sort_by_key(|app| app.name.to_lowercase());
        Ok(apps)
    }

    fn invoke(&self, command: &str) -> Result<Output, DeskActError> {
        run_agent_desktop(&self.executable, command)
    }

    /// Invoke a native CLI subcommand with the standard native-process timeout.
    fn invoke_args(&self, args: &[&str], command_name: &str) -> Result<Output, DeskActError> {
        self.invoke_args_with_timeout(args, command_name, NATIVE_PROCESS_TIMEOUT)
    }

    /// Invoke a native CLI subcommand with a caller-provided hard deadline.
    fn invoke_args_with_timeout(
        &self,
        args: &[&str],
        command_name: &str,
        timeout: Duration,
    ) -> Result<Output, DeskActError> {
        let mut command = Command::new(&self.executable);
        command.args(args);
        match run_command_with_timeout(&mut command, timeout) {
            Ok(output) => Ok(output),
            Err(CommandRunError::TimedOut) => Err(DeskActError(format!(
                "agent-desktop `{command_name}` timed out after {} milliseconds",
                timeout.as_millis()
            ))),
            Err(CommandRunError::Io(error)) => Err(DeskActError(format!(
                "Could not start agent-desktop `{command_name}`: {error}. {INSTALL_GUIDANCE}"
            ))),
        }
    }

    /// Launch a selected catalog entry, then identify its window from fresh inventory.
    pub fn resolve_target(
        &self,
        app: DiscoveredApp,
    ) -> Result<NativeTargetResolution, DeskActError> {
        let target = app.target.trim();
        if target.is_empty() {
            return Err(DeskActError(format!(
                "The discovered target for `{}` is empty; no app was launched.",
                app.name
            )));
        }
        if !app.arguments.trim().is_empty() {
            return Err(DeskActError(format!(
                "The discovered shortcut for `{}` requires arguments that this harness does not yet parse safely. No app was launched.",
                app.name
            )));
        }
        let mut launch_args = vec!["launch".to_owned(), target.to_owned()];
        if !app.working_directory.trim().is_empty() {
            launch_args.push("--cwd".into());
            launch_args.push(app.working_directory.clone());
        }
        let launch_args: Vec<_> = launch_args.iter().map(String::as_str).collect();
        let launch = self.invoke_args(&launch_args, "launch")?;
        let launch: Envelope<serde_json::Value> = parse_envelope(&launch, "launch")?;
        if launch.version != SUPPORTED_JSON_CONTRACT_VERSION {
            return Err(DeskActError(format!(
                "Unsupported agent-desktop JSON contract {} in launch response (DeskAct requires {}).",
                launch.version, SUPPORTED_JSON_CONTRACT_VERSION
            )));
        }
        if !launch.ok {
            return Err(cli_error("launch", launch.error));
        }
        self.resolve_window_state(app, Some(Instant::now() + APP_READINESS_TIMEOUT))
    }

    /// Resume after the user restores a minimized app; never relaunches it.
    pub fn resume_after_manual_restore(
        &self,
        app: DiscoveredApp,
    ) -> Result<NativeTargetResolution, DeskActError> {
        self.resolve_window_state(app, None)
    }

    /// Re-enumerate windows and resolve one owner-verified accessible target.
    fn resolve_window_state(
        &self,
        app: DiscoveredApp,
        readiness_deadline: Option<Instant>,
    ) -> Result<NativeTargetResolution, DeskActError> {
        let expected_process = executable_stem(&app.target);
        let mut backoff = Duration::from_millis(100);
        let matching = loop {
            let output = if let Some(deadline) = readiness_deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(no_matching_window_error(&app, &expected_process));
                }
                let timeout_ms = remaining.as_millis().max(1).to_string();
                self.invoke_args_with_timeout(
                    &["list-windows", "--timeout-ms", &timeout_ms],
                    "list-windows",
                    remaining,
                )?
            } else {
                self.invoke("list-windows")?
            };
            let envelope: Envelope<Vec<NativeWindow>> = parse_envelope(&output, "list-windows")?;
            if envelope.version != SUPPORTED_JSON_CONTRACT_VERSION {
                return Err(DeskActError(format!(
                    "Unsupported agent-desktop JSON contract {} in window inventory (DeskAct requires {}).",
                    envelope.version, SUPPORTED_JSON_CONTRACT_VERSION
                )));
            }
            if !envelope.ok {
                return Err(cli_error("list-windows", envelope.error));
            }
            let windows = envelope.data.ok_or_else(|| {
                DeskActError("agent-desktop window inventory did not include data".into())
            })?;
            let matching: Vec<_> = windows
                .into_iter()
                .filter(|window| {
                    executable_stem(&window.app_name).eq_ignore_ascii_case(&expected_process)
                })
                .collect();
            if !matching.is_empty() {
                break matching;
            }
            let Some(deadline) = readiness_deadline else {
                return Err(no_matching_window_error(&app, &expected_process));
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(no_matching_window_error(&app, &expected_process));
            }
            thread::sleep(backoff.min(remaining));
            backoff = (backoff * 2).min(Duration::from_secs(1));
        };
        let accessible: Vec<_> = matching
            .into_iter()
            .filter(|window| window.accessible)
            .collect();
        if accessible.is_empty() {
            return Err(DeskActError(format!(
                "No matching window for `{}` exposes accessible controls. No action is permitted.",
                app.name
            )));
        }
        let mut owner_verified = Vec::new();
        for window in accessible {
            if window.pid == 0 {
                return Err(DeskActError(format!(
                    "The owner PID for window `{}` could not be determined. Hosted-app ownership cannot be verified; no window was selected.",
                    window.id
                )));
            }
            let image_path = process_image_path(window.pid)?;
            if !windows_paths_equal(&image_path, &app.target) {
                continue;
            }
            owner_verified.push(window);
        }
        if owner_verified.is_empty() {
            return Err(DeskActError(format!(
                "No accessible `{}` window has an owner executable matching `{}`. A hosted-app/container window may report its host process; ownership could not be verified, so no window was selected.",
                app.name, app.target
            )));
        }
        if owner_verified.len() != 1 {
            return Err(DeskActError(format!(
                "Found {} accessible windows owned by `{}`; exactly one is required. No window was selected.",
                owner_verified.len(),
                app.target
            )));
        }
        let window = owner_verified
            .into_iter()
            .next()
            .expect("one verified owner");
        if window.minimized {
            return Ok(NativeTargetResolution::NeedsManualRestore { app, window });
        }

        let output = self.invoke_args(
            &[
                "snapshot",
                "--app",
                &window.app_name,
                "--window-id",
                &window.id,
                "-i",
            ],
            "snapshot",
        )?;
        let snapshot: Envelope<serde_json::Value> = parse_envelope(&output, "snapshot")?;
        if snapshot.version != SUPPORTED_JSON_CONTRACT_VERSION {
            return Err(DeskActError(format!(
                "Unsupported agent-desktop JSON contract {} in accessibility snapshot (DeskAct requires {}).",
                snapshot.version, SUPPORTED_JSON_CONTRACT_VERSION
            )));
        }
        if !snapshot.ok {
            return Err(cli_error("snapshot", snapshot.error));
        }
        let data = snapshot.data.ok_or_else(|| {
            DeskActError("agent-desktop accessibility snapshot did not include data".into())
        })?;
        let snapshot_window_id = data
            .pointer("/window/id")
            .and_then(serde_json::Value::as_str);
        if snapshot_window_id != Some(window.id.as_str()) {
            return Err(DeskActError(format!(
                "Fresh accessibility snapshot did not confirm the selected window `{}`. No target was returned.",
                window.id
            )));
        }
        if data.get("complete").and_then(serde_json::Value::as_bool) != Some(true) {
            return Err(DeskActError(format!(
                "Accessibility snapshot for `{}` was incomplete. No target was returned.",
                window.title
            )));
        }
        let ref_count = data
            .get("ref_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize;
        if ref_count == 0 {
            return Err(DeskActError(format!(
                "Accessibility snapshot for `{}` exposed no actionable controls. No target was returned.",
                window.title
            )));
        }
        let snapshot_id = data
            .get("snapshot_id")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                DeskActError("Accessibility snapshot did not include a snapshot identifier".into())
            })?
            .to_owned();

        Ok(NativeTargetResolution::Ready(ResolvedNativeTarget {
            app,
            window,
            snapshot_id,
            ref_count,
        }))
    }

    /// Capture and validate a fresh interactive snapshot for one selected window.
    pub(crate) fn fresh_snapshot(
        &self,
        window: &NativeWindow,
    ) -> Result<serde_json::Value, DeskActError> {
        self.fresh_snapshot_with_timeout(window, NATIVE_PROCESS_TIMEOUT)
    }

    /// Capture a fresh interactive snapshot without exceeding the supplied time budget.
    pub(crate) fn fresh_snapshot_with_timeout(
        &self,
        window: &NativeWindow,
        timeout: Duration,
    ) -> Result<serde_json::Value, DeskActError> {
        let output = self.invoke_args_with_timeout(
            &[
                "snapshot",
                "--app",
                &window.app_name,
                "--window-id",
                &window.id,
                "-i",
            ],
            "snapshot",
            timeout,
        )?;
        let data = successful_data(output, "snapshot")?;
        if data
            .pointer("/window/id")
            .and_then(serde_json::Value::as_str)
            != Some(&window.id)
        {
            return Err(DeskActError(format!(
                "Snapshot did not confirm selected window `{}`",
                window.id
            )));
        }
        if data.get("complete").and_then(serde_json::Value::as_bool) != Some(true)
            || data
                .get("snapshot_id")
                .and_then(serde_json::Value::as_str)
                .is_none_or(str::is_empty)
            || data
                .get("ref_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0)
                == 0
        {
            return Err(DeskActError(format!(
                "Fresh snapshot for `{}` was incomplete or exposed no actionable controls",
                window.title
            )));
        }
        Ok(data)
    }

    /// Invoke a native CLI subcommand and return its validated success data.
    pub(crate) fn invoke_success(
        &self,
        args: &[&str],
        command: &str,
    ) -> Result<serde_json::Value, DeskActError> {
        let output = self.invoke_args(args, command)?;
        successful_data(output, command)
    }

    /// Invoke a native CLI subcommand and return the `value` field of its success data.
    pub(crate) fn invoke_value(
        &self,
        args: &[&str],
        command: &str,
    ) -> Result<serde_json::Value, DeskActError> {
        let data = self.invoke_success(args, command)?;
        data.get("value").cloned().ok_or_else(|| {
            DeskActError(format!(
                "agent-desktop `{command}` result did not include a value"
            ))
        })
    }

    /// Send a keyboard shortcut to one Notepad window.
    pub(crate) fn press_shortcut(
        &self,
        window: &NativeWindow,
        shortcut: &str,
    ) -> Result<(), DeskActError> {
        let args = [
            "press",
            "--headed",
            "--app",
            "notepad",
            "--window-id",
            &window.id,
            shortcut,
        ];
        self.invoke_success(&args, "press")?;
        Ok(())
    }

    /// Focus one verified window before sending a keyboard shortcut to that app.
    pub(crate) fn press_app_shortcut(
        &self,
        window: &NativeWindow,
        app: &str,
        shortcut: &str,
    ) -> Result<(), DeskActError> {
        let focused = self.invoke_success(
            &["focus-window", "--app", app, "--window-id", &window.id],
            "focus-window",
        )?;
        if focused
            .pointer("/focused/id")
            .and_then(serde_json::Value::as_str)
            != Some(window.id.as_str())
            || focused
                .pointer("/focused/is_focused")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
        {
            return Err(DeskActError(format!(
                "agent-desktop did not confirm focus on window `{}` before sending `{shortcut}`",
                window.id
            )));
        }
        let args = [
            "press",
            "--headed",
            "--app",
            app,
            "--window-id",
            &window.id,
            shortcut,
        ];
        self.invoke_success(&args, "press")?;
        Ok(())
    }
}

/// Explain that no fresh window could be associated with the requested app before timeout.
fn no_matching_window_error(app: &DiscoveredApp, expected_process: &str) -> DeskActError {
    DeskActError(format!(
        "No fresh window matched `{}` (target process `{expected_process}`) before the app-readiness deadline. Hosted-app windows may expose only their container process; ownership could not be verified, so no window was selected.",
        app.name
    ))
}

/// Compare Windows executable paths using their case-insensitive, separator-insensitive form.
pub(crate) fn windows_paths_equal(left: &str, right: &str) -> bool {
    fn normalize(path: &str) -> String {
        let path = path.trim().trim_matches('"').replace('/', "\\");
        path.strip_prefix("\\\\?\\").unwrap_or(&path).to_lowercase()
    }

    normalize(left) == normalize(right)
}

/// Extract a target's file stem for a preliminary process-label filter.
pub(crate) fn executable_stem(target: &str) -> String {
    let file_name = target.rsplit(['\\', '/']).next().unwrap_or(target);
    file_name
        .rsplit_once('.')
        .map(|(stem, _)| stem)
        .unwrap_or(file_name)
        .to_owned()
}

#[derive(Deserialize)]
struct Envelope<T> {
    version: String,
    ok: bool,
    command: String,
    data: Option<T>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct VersionData {
    version: String,
    os: String,
    target: String,
}

#[derive(Deserialize)]
struct AppInventory {
    apps: Vec<RunningApp>,
}

/// Verify the installed native automation CLI and its structured-output contract.
pub fn preflight() -> Result<PreflightStatus, DeskActError> {
    let cli = VerifiedNativeCli::connect()?;
    Ok(cli.preflight_status().clone())
}

/// Return installed app entries and currently running GUI apps after successful preflight.
pub fn native_inventory() -> Result<NativeInventory, DeskActError> {
    let cli = VerifiedNativeCli::connect()?;
    let preflight = cli.preflight_status().clone();
    let running_apps = cli.running_apps()?;
    let discovery = windows_catalog::discover_app_catalog()?;
    let catalog = normalize_catalog(discovery.entries);

    Ok(NativeInventory {
        preflight,
        catalog,
        skipped_start_menu_shortcuts: discovery.skipped_start_menu_shortcuts,
        running_apps,
    })
}

fn successful_data(output: Output, command: &str) -> Result<serde_json::Value, DeskActError> {
    let envelope: Envelope<serde_json::Value> = parse_envelope(&output, command)?;
    if envelope.version != SUPPORTED_JSON_CONTRACT_VERSION {
        return Err(DeskActError(format!(
            "Unsupported agent-desktop JSON contract {} in `{command}` response (DeskAct requires {}).",
            envelope.version, SUPPORTED_JSON_CONTRACT_VERSION
        )));
    }
    if !envelope.ok {
        return Err(cli_error(command, envelope.error));
    }
    envelope.data.ok_or_else(|| {
        DeskActError(format!(
            "agent-desktop `{command}` response did not include data"
        ))
    })
}

/// Launch a uniquely resolved app and identify its fresh accessible window.
pub fn resolve_native_target(app_name: &str) -> Result<NativeTargetResolution, DeskActError> {
    let cli = VerifiedNativeCli::connect()?;
    let catalog = load_app_catalog()?;
    match prepare_app_selection(&catalog, app_name)? {
        AppSelectionInput::ExactMatch { entry } => cli.resolve_target(entry.as_discovered_app()),
        AppSelectionInput::NeedsClarification { candidates } => Err(DeskActError(format!(
            "More than one catalog entry exactly matches `{}` ({} candidates); ask the user which app they mean. No app was launched.",
            app_name.trim(),
            candidates.len()
        ))),
        AppSelectionInput::NeedsModel { .. } => Err(DeskActError(format!(
            "The app request `{}` needs primary-model selection from the catalog. No app was launched; use `deskact-dev app-catalog prepare` to inspect the safe candidate set.",
            app_name.trim()
        ))),
    }
}

/// Sort and remove only catalog entries with identical launch configuration.
pub(crate) fn normalize_catalog(mut catalog: Vec<DiscoveredApp>) -> Vec<DiscoveredApp> {
    catalog.sort_by_key(|app| {
        (
            app.name.to_lowercase(),
            app.target.to_lowercase(),
            app.arguments.clone(),
            app.working_directory.to_lowercase(),
        )
    });
    catalog.dedup_by(|left, right| {
        left.name.eq_ignore_ascii_case(&right.name)
            && left.target.eq_ignore_ascii_case(&right.target)
            && left.arguments == right.arguments
            && left
                .working_directory
                .eq_ignore_ascii_case(&right.working_directory)
    });
    catalog
}

/// Parse a successful CLI envelope, preserving structured errors from nonzero exits.
fn parse_envelope<T: for<'de> Deserialize<'de>>(
    output: &Output,
    requested_command: &str,
) -> Result<Envelope<T>, DeskActError> {
    if !output.status.success() {
        if let Ok(response) = serde_json::from_slice::<Envelope<serde_json::Value>>(&output.stdout)
            && response.command == requested_command
            && !response.ok
        {
            return Err(cli_error(requested_command, response.error));
        }
        let details = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(DeskActError(if details.is_empty() {
            format!(
                "agent-desktop `{requested_command}` exited with status {}",
                output.status
            )
        } else {
            format!("agent-desktop `{requested_command}` failed: {details}")
        }));
    }

    let response: Envelope<T> = serde_json::from_slice(&output.stdout).map_err(|error| {
        DeskActError(format!(
            "agent-desktop `{requested_command}` returned invalid JSON: {error}"
        ))
    })?;
    if response.command != requested_command {
        return Err(DeskActError(format!(
            "agent-desktop returned command `{}` for a `{requested_command}` request",
            response.command
        )));
    }
    Ok(response)
}

fn cli_error(command: &str, error: Option<serde_json::Value>) -> DeskActError {
    let details = error
        .map(|value| value.to_string())
        .unwrap_or_else(|| "no error details provided".into());
    DeskActError(format!("agent-desktop `{command}` failed: {details}"))
}

fn run_agent_desktop(executable: &OsStr, command: &str) -> Result<Output, DeskActError> {
    let mut process = Command::new(executable);
    process.arg(command);
    match run_command_with_timeout(&mut process, NATIVE_PROCESS_TIMEOUT) {
        Ok(output) => Ok(output),
        Err(CommandRunError::TimedOut) => Err(DeskActError(format!(
            "agent-desktop `{command}` timed out after {} seconds",
            NATIVE_PROCESS_TIMEOUT.as_secs()
        ))),
        Err(CommandRunError::Io(error)) => {
            let executable = executable.to_string_lossy();
            if error.kind() == io::ErrorKind::NotFound {
                Err(DeskActError(format!(
                    "Could not find or start agent-desktop ({executable}). Set DESKACT_AGENT_DESKTOP to the agent-desktop-win32-x64.exe path for non-default npm installations. {INSTALL_GUIDANCE}"
                )))
            } else {
                Err(DeskActError(format!(
                    "Could not start agent-desktop ({executable}): {error}. {INSTALL_GUIDANCE}"
                )))
            }
        }
    }
}

/// Wait for a subprocess without inheritable output pipes and enforce a hard deadline.
fn run_command_status_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<ExitStatus, CommandRunError> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(CommandRunError::Io)?;
    let deadline = Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = terminate_and_reap(&mut child);
                return Err(CommandRunError::TimedOut);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = terminate_and_reap(&mut child);
                return Err(CommandRunError::Io(error));
            }
        }
    }
}

/// Run a subprocess while draining both output streams and enforcing a hard deadline.
pub(crate) fn run_command_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<Output, CommandRunError> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(CommandRunError::Io)?;
    let stdout_reader = drain_pipe(child.stdout.take().expect("stdout was piped"));
    let stderr_reader = drain_pipe(child.stderr.take().expect("stderr was piped"));
    let deadline = Instant::now() + timeout;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(Output {
                    status,
                    stdout: finish_pipe(stdout_reader)?,
                    stderr: finish_pipe(stderr_reader)?,
                });
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = terminate_and_reap(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(CommandRunError::TimedOut);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = terminate_and_reap(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(CommandRunError::Io(error));
            }
        }
    }
}

fn terminate_and_reap(child: &mut Child) -> io::Result<()> {
    let kill_result = child.kill();
    let wait_result = child.wait();
    kill_result.and(wait_result.map(|_| ()))
}

fn drain_pipe<R: Read + Send + 'static>(mut pipe: R) -> JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut output = Vec::new();
        pipe.read_to_end(&mut output)?;
        Ok(output)
    })
}

fn finish_pipe(reader: JoinHandle<io::Result<Vec<u8>>>) -> Result<Vec<u8>, CommandRunError> {
    reader
        .join()
        .map_err(|_| CommandRunError::Io(io::Error::other("subprocess output reader panicked")))?
        .map_err(CommandRunError::Io)
}

fn configured_executable() -> OsString {
    if let Some(path) = env::var_os("DESKACT_AGENT_DESKTOP") {
        return path;
    }

    #[cfg(windows)]
    if let Some(app_data) = env::var_os("APPDATA") {
        let executable = std::path::PathBuf::from(app_data)
            .join("npm")
            .join("node_modules")
            .join("agent-desktop")
            .join("bin")
            .join("agent-desktop-win32-x64.exe");
        if executable.is_file() {
            return executable.into_os_string();
        }
    }

    OsString::from("agent-desktop")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ensure the generic pipe-draining subprocess runner kills a process at its deadline.
    #[cfg(unix)]
    #[test]
    fn subprocess_deadline_terminates_and_reaps_child() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 10"]);
        let started = Instant::now();

        let result = run_command_with_timeout(&mut command, Duration::from_millis(50));

        assert!(matches!(result, Err(CommandRunError::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// Verify launch waits avoid inherited output pipes and terminate a hung direct child.
    #[cfg(unix)]
    #[test]
    fn status_wait_is_bounded_and_does_not_wait_for_inherited_output_handles() {
        let unique = format!("deskact-child-{}", std::process::id());
        let pid_file = std::env::temp_dir().join(unique);
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "sleep 30 & echo $! > \"$DESKACT_TEST_CHILD_PID_FILE\"; exit 0",
            ])
            .env("DESKACT_TEST_CHILD_PID_FILE", &pid_file);
        let started = Instant::now();
        let status = run_command_status_with_timeout(&mut command, Duration::from_secs(2))
            .expect("launcher parent should exit successfully");
        assert!(status.success());
        assert!(started.elapsed() < Duration::from_secs(2));
        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            let _ = Command::new("kill").arg(pid.trim()).status();
            let _ = std::fs::remove_file(pid_file);
        }

        let mut hung = Command::new("sh");
        hung.args(["-c", "exec sleep 10"]);
        let started = Instant::now();
        assert!(matches!(
            run_command_status_with_timeout(&mut hung, Duration::from_millis(50)),
            Err(CommandRunError::TimedOut)
        ));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// A poll returns the first attempt that yields a value, passing the time left each time.
    #[test]
    fn poll_returns_first_ready_value() {
        let mut calls = 0;
        let value = poll_until_deadline(
            Instant::now() + Duration::from_secs(5),
            |remaining| {
                assert!(!remaining.is_zero());
                calls += 1;
                Ok((calls == 3).then_some(calls))
            },
            || Err(DeskActError("timed out".into())),
        )
        .expect("third attempt is ready");

        assert_eq!(value, 3);
    }

    /// A poll that never becomes ready hands control to the timeout outcome at the deadline.
    #[test]
    fn poll_reports_timeout_at_deadline() {
        let started = Instant::now();

        let error = poll_until_deadline::<()>(
            started + Duration::from_millis(250),
            |_| Ok(None),
            || Err(DeskActError("timed out".into())),
        )
        .expect_err("nothing becomes ready");

        assert_eq!(error.to_string(), "timed out");
        assert!(started.elapsed() >= Duration::from_millis(250));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    /// The timeout outcome may supply a fallback value after one attempt on an already-passed deadline.
    #[test]
    fn poll_attempts_once_then_times_out_with_a_fallback_after_deadline() {
        let mut calls = 0;
        let value = poll_until_deadline(
            Instant::now(),
            |remaining| -> Result<Option<u8>, DeskActError> {
                assert!(!remaining.is_zero());
                calls += 1;
                Ok(None)
            },
            || Ok(7),
        )
        .expect("fallback value");

        assert_eq!(value, 7);
        assert_eq!(calls, 1);
    }

    /// An already-passed deadline still lets a ready attempt win.
    #[test]
    fn poll_attempts_once_even_when_the_deadline_has_passed() {
        let value =
            poll_until_deadline(Instant::now(), |_| Ok(Some(1u8)), || Ok(7)).expect("ready value");

        assert_eq!(value, 1);
    }

    /// Attempt failures abort the poll immediately.
    #[test]
    fn poll_propagates_attempt_errors() {
        let mut calls = 0;
        let error = poll_until_deadline::<()>(
            Instant::now() + Duration::from_secs(5),
            |_| {
                calls += 1;
                Err(DeskActError("attempt failed".into()))
            },
            || Err(DeskActError("timed out".into())),
        )
        .expect_err("attempt fails");

        assert_eq!(error.to_string(), "attempt failed");
        assert_eq!(calls, 1);
    }
}
