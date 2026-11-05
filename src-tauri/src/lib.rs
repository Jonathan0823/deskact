//! Native CLI preflight, Windows application inventory, and controlled developer acceptance workflows for DeskAct.

use std::{
    env,
    ffi::{OsStr, OsString},
    fmt, fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::Deserialize;

mod windows_catalog;
pub use windows_catalog::{
    CatalogDiscovery, DiscoveredApp, DiscoverySource, discover_app_catalog, process_image_path,
};

/// Exact `agent-desktop` release required for native CLI operations.
pub const SUPPORTED_AGENT_DESKTOP_VERSION: &str = "1.1.0";
/// Structured JSON contract version required from `agent-desktop`.
pub const SUPPORTED_JSON_CONTRACT_VERSION: &str = "2.6";
const INSTALL_GUIDANCE: &str = "Install the required version from Windows PowerShell with `npm install --global agent-desktop@1.1.0`; DeskAct never installs or updates automation tools.";
pub(crate) const NATIVE_PROCESS_TIMEOUT: Duration = Duration::from_secs(30);
/// Maximum time to wait after activation for the target window to become enumerable.
pub const APP_READINESS_TIMEOUT: Duration = Duration::from_secs(10);

/// Failure to start a subprocess or complete it before the native-operation deadline.
#[derive(Debug)]
pub(crate) enum CommandRunError {
    Io(io::Error),
    TimedOut,
}

/// Verified versions reported by the installed native automation CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightStatus {
    /// Exact installed `agent-desktop` version.
    pub tool_version: String,
    /// Structured JSON contract version reported by the CLI.
    pub contract_version: String,
}

/// A currently running GUI application reported by `agent-desktop`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunningApp {
    /// Application or process name.
    pub name: String,
    /// Operating-system process identifier.
    pub pid: u32,
    /// Whether the window is foreground or background, as reported by the CLI.
    pub presentation: String,
}

/// Read-only snapshot of discovered application entries and running GUI apps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeInventory {
    /// Verified native CLI versions.
    pub preflight: PreflightStatus,
    /// Deterministically ordered discovered app entries.
    pub catalog: Vec<DiscoveredApp>,
    /// Number of Start Menu shortcuts skipped because they lacked a file target.
    pub skipped_start_menu_shortcuts: usize,
    /// Currently running GUI applications.
    pub running_apps: Vec<RunningApp>,
}

/// A fresh, uniquely matched native window with an accessibility snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNativeTarget {
    /// Discovered catalog entry selected by its exact display name (case-insensitive).
    pub app: DiscoveredApp,
    /// Window identity from the fresh `list-windows` response.
    pub window: NativeWindow,
    /// Identifier of the fresh complete accessibility snapshot.
    pub snapshot_id: String,
    /// Number of actionable accessibility references in the snapshot.
    pub ref_count: usize,
}

/// Window returned by `agent-desktop list-windows`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NativeWindow {
    /// Stable window identifier from the fresh window inventory.
    pub id: String,
    /// Visible window title reported by the native automation tool.
    pub title: String,
    /// Process/app label reported for the window; not proof of executable ownership.
    pub app_name: String,
    /// Whether the window exposes semantic accessibility controls.
    pub accessible: bool,
    /// Whether the window is currently minimized.
    pub minimized: bool,
    /// Whether the window is visible, when reported by the CLI.
    #[serde(default)]
    pub visible: bool,
    /// Current operating-system process id reported for the window.
    #[serde(default)]
    pub pid: u32,
}

/// Resolution may pause so the user can manually restore a minimized window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeTargetResolution {
    /// A uniquely matched, owner-verified accessible window has a complete snapshot.
    Ready(ResolvedNativeTarget),
    /// The user must restore the selected window manually before resolution can continue.
    NeedsManualRestore {
        /// The selected discovered app entry.
        app: DiscoveredApp,
        /// The minimized window to restore.
        window: NativeWindow,
    },
}

/// Error returned when native CLI preflight or inventory cannot be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskActError(String);

impl DeskActError {
    /// Construct an error from an actionable diagnostic message.
    pub(crate) fn new(message: String) -> Self {
        Self(message)
    }
}

impl fmt::Display for DeskActError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DeskActError {}

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
    fn windows_for_app(&self, app: &str) -> Result<Vec<NativeWindow>, DeskActError> {
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
}

/// Explain that no fresh window could be associated with the requested app before timeout.
fn no_matching_window_error(app: &DiscoveredApp, expected_process: &str) -> DeskActError {
    DeskActError(format!(
        "No fresh window matched `{}` (target process `{expected_process}`) before the app-readiness deadline. Hosted-app windows may expose only their container process; ownership could not be verified, so no window was selected.",
        app.name
    ))
}

/// Compare Windows executable paths using their case-insensitive, separator-insensitive form.
fn windows_paths_equal(left: &str, right: &str) -> bool {
    fn normalize(path: &str) -> String {
        let path = path.trim().trim_matches('"').replace('/', "\\");
        path.strip_prefix("\\\\?\\").unwrap_or(&path).to_lowercase()
    }

    normalize(left) == normalize(right)
}

/// Extract a target's file stem for a preliminary process-label filter.
fn executable_stem(target: &str) -> String {
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

/// Run controlled Notepad acceptance scenarios in isolated, newly created tabs.
pub fn run_notepad_acceptance(runs: usize) -> Result<NotepadAcceptanceReport, DeskActError> {
    if runs == 0 {
        return Err(DeskActError(
            "The Notepad acceptance run count must be greater than zero".into(),
        ));
    }
    let cli = VerifiedNativeCli::connect()?;
    let mut failures = Vec::new();
    let mut attempted = 0;
    let mut verified = 0;
    for run in 1..=runs {
        attempted += 1;
        match run_notepad_scenario(&cli, run) {
            Ok(()) => verified += 1,
            Err(failure) => {
                failures.push(format!("run {run}: {}", failure.error));
                if !failure.safe_to_continue {
                    break;
                }
            }
        }
    }
    let required = required_successes(runs);
    if verified < required {
        return Err(DeskActError(format!(
            "Notepad acceptance verified {verified}/{attempted} attempted runs ({runs} requested); {required} verified runs are required. Failures: {}",
            failures.join("; ")
        )));
    }
    Ok(NotepadAcceptanceReport {
        attempted,
        verified,
        gate_met: runs >= 20 && verified >= required,
        failures,
    })
}

fn required_successes(runs: usize) -> usize {
    (runs / 100) * 95 + (runs % 100 * 95).div_ceil(100)
}

/// Per-run evidence summary for the controlled Notepad acceptance command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotepadAcceptanceReport {
    /// Number of scenarios attempted.
    pub attempted: usize,
    /// Number of scenarios independently verified end to end.
    pub verified: usize,
    /// Whether the 20-run, 95% acceptance gate was met.
    pub gate_met: bool,
    /// Diagnostics for scenarios that did not verify.
    pub failures: Vec<String>,
}

static ACCEPTANCE_PATH_COUNTER: AtomicU64 = AtomicU64::new(0);

struct AcceptanceArtifacts {
    directory: PathBuf,
    file: PathBuf,
}

impl AcceptanceArtifacts {
    fn create(run: usize) -> Result<Self, DeskActError> {
        for _ in 0..100 {
            let counter = ACCEPTANCE_PATH_COUNTER.fetch_add(1, Ordering::Relaxed);
            let unique_id = format!(
                "{}-{}-{run}-{counter}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            );
            let directory = env::temp_dir().join(format!("deskact-notepad-{unique_id}"));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    let file = directory.join(format!("deskact-marker-{unique_id}.txt"));
                    return Ok(Self { directory, file });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(DeskActError(format!(
                        "Could not create a unique Notepad acceptance directory: {error}"
                    )));
                }
            }
        }
        Err(DeskActError(
            "Could not allocate a unique Notepad acceptance directory after 100 attempts".into(),
        ))
    }
}

impl AcceptanceArtifacts {
    fn cleanup(self) -> Result<(), DeskActError> {
        match fs::remove_file(&self.file) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(DeskActError(format!(
                    "Could not remove the test-created Notepad acceptance file: {error}"
                )));
            }
        }
        match fs::remove_dir(&self.directory) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(DeskActError(format!(
                "Could not remove the unique Notepad acceptance directory: {error}"
            ))),
        }
    }
}

impl Drop for AcceptanceArtifacts {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
        let _ = fs::remove_dir(&self.directory);
    }
}

struct ScenarioFailure {
    error: DeskActError,
    safe_to_continue: bool,
}

fn run_notepad_scenario(cli: &VerifiedNativeCli, run: usize) -> Result<(), ScenarioFailure> {
    let mut ui_may_have_changed = false;
    run_notepad_scenario_inner(cli, run, &mut ui_may_have_changed).map_err(|error| {
        ScenarioFailure {
            error,
            safe_to_continue: !ui_may_have_changed,
        }
    })
}

fn run_notepad_scenario_inner(
    cli: &VerifiedNativeCli,
    run: usize,
    ui_may_have_changed: &mut bool,
) -> Result<(), DeskActError> {
    let catalog = normalize_catalog(windows_catalog::discover_app_catalog()?.entries);
    let apps: Vec<_> = catalog
        .into_iter()
        .filter(|app| app.name.eq_ignore_ascii_case("notepad"))
        .collect();
    let app = match apps.as_slice() {
        [app] => app,
        [] => {
            return Err(DeskActError(
                "No uniquely discovered Notepad executable is available".into(),
            ));
        }
        _ => {
            return Err(DeskActError(
                "Multiple Notepad launch targets were discovered; no window was selected".into(),
            ));
        }
    };
    let windows = cli.windows_for_app("notepad")?;
    if windows.len() != 1 {
        return Err(DeskActError(format!(
            "Expected exactly one existing Notepad window; found {}. No document was opened or modified.",
            windows.len()
        )));
    }
    let original = windows.into_iter().next().expect("one window checked");
    if !original.accessible || original.minimized {
        return Err(DeskActError(format!(
            "Notepad window `{}` is inaccessible or minimized; no action was taken.",
            original.id
        )));
    }
    if original.title.eq_ignore_ascii_case("Untitled - Notepad") {
        return Err(DeskActError(
            "The existing active Notepad tab is already titled `Untitled - Notepad`, so a fresh Ctrl+N tab cannot be distinguished safely; no action was taken.".into(),
        ));
    }
    if original.pid == 0 {
        return Err(DeskActError(format!(
            "Notepad window `{}` has no verifiable owner PID; no action was taken.",
            original.id
        )));
    }
    let owner_path = process_image_path(original.pid)?;
    if !windows_paths_equal(&owner_path, &app.target) {
        return Err(DeskActError(format!(
            "Notepad window `{}` is not owned by the uniquely discovered Notepad executable; no action was taken.",
            original.id
        )));
    }

    let artifacts = AcceptanceArtifacts::create(run)?;
    let marker = "DESKACT_NOTEPAD_ACCEPTANCE_MARKER".to_owned();
    let filename = artifacts
        .file
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| DeskActError("The unique acceptance filename is not valid Unicode".into()))?
        .to_owned();
    *ui_may_have_changed = true;
    press_shortcut(cli, &original, "ctrl+n")?;
    let new_tab = wait_for_window_title(cli, &original, "Untitled - Notepad", &original.title)?;
    if !new_tab.title.eq_ignore_ascii_case("Untitled - Notepad") {
        return Err(DeskActError(format!(
            "Ctrl+N did not produce the expected fresh `Untitled - Notepad` tab (observed `{}`). It was not closed.",
            new_tab.title
        )));
    }
    let editor_snapshot = fresh_snapshot(cli, &new_tab)?;
    let editor = unique_control(&editor_snapshot, "textfield", None, "new-tab editor")?;
    let editor_ref = control_ref(editor)?;
    let initial_value = invoke_value(cli, &["get", editor_ref, "--property", "value"], "get")?;
    if initial_value.as_str() != Some("") {
        return Err(DeskActError(
            "Fresh Notepad tab was not independently verified empty; no text was changed".into(),
        ));
    }
    invoke_success(cli, &["set-value", editor_ref, &marker], "set-value")?;

    press_shortcut(cli, &new_tab, "ctrl+s")?;
    let dialog = wait_for_save_dialog(cli, &original)?;
    let save_snapshot = fresh_snapshot(cli, &dialog)?;
    let filename_field = unique_control(
        &save_snapshot,
        "textfield",
        Some("File name:"),
        "Save As filename field",
    )?;
    let filename_ref = control_ref(filename_field)?;
    let full_path = artifacts.file.to_str().ok_or_else(|| {
        DeskActError(
            "The unique acceptance path is not valid Unicode; no save was attempted".into(),
        )
    })?;
    invoke_success(cli, &["set-value", filename_ref, full_path], "set-value")?;

    let save_snapshot = fresh_snapshot(cli, &dialog)?;
    let filename_field = unique_control(
        &save_snapshot,
        "textfield",
        Some("File name:"),
        "Save As filename field",
    )?;
    if filename_field
        .get("value")
        .and_then(serde_json::Value::as_str)
        != Some(full_path)
    {
        return Err(DeskActError(
            "Fresh Save As observation did not confirm the unique output path; nothing was saved"
                .into(),
        ));
    }
    let save_button = unique_control(&save_snapshot, "button", Some("Save"), "Save button")?;
    let save_ref = control_ref(save_button)?;
    invoke_success(cli, &["click", save_ref], "click")?;
    match wait_for_saved_contents(
        cli,
        &original,
        &dialog,
        &artifacts.file,
        &marker,
        Duration::from_secs(2),
    )? {
        SaveStatus::Saved => {}
        SaveStatus::DialogStillOpen => {
            if fs::symlink_metadata(&artifacts.file).is_ok() {
                let contents = fs::read_to_string(&artifacts.file).map_err(|error| {
                    DeskActError(format!(
                        "Saved acceptance file could not be read independently: {error}"
                    ))
                })?;
                if contents != marker {
                    return Err(DeskActError(
                        "The unique acceptance path appeared with unexpected contents; no second save was attempted".into(),
                    ));
                }
                if wait_for_saved_contents(
                    cli,
                    &original,
                    &dialog,
                    &artifacts.file,
                    &marker,
                    Duration::from_secs(5),
                )? != SaveStatus::Saved
                {
                    return Err(DeskActError(
                        "The verified file was saved, but the Save As dialog did not close".into(),
                    ));
                }
            } else {
                let save_snapshot = fresh_snapshot(cli, &dialog)?;
                let filename_field = unique_control(
                    &save_snapshot,
                    "textfield",
                    Some("File name:"),
                    "Save As filename field",
                )?;
                if filename_field
                    .get("value")
                    .and_then(serde_json::Value::as_str)
                    != Some(full_path)
                {
                    return Err(DeskActError(
                        "Save As no longer points to the unique output path; no retry was attempted"
                            .into(),
                    ));
                }
                let save_button =
                    unique_control(&save_snapshot, "button", Some("Save"), "Save button")?;
                invoke_success(cli, &["click", control_ref(save_button)?], "click")?;
                if wait_for_saved_contents(
                    cli,
                    &original,
                    &dialog,
                    &artifacts.file,
                    &marker,
                    Duration::from_secs(5),
                )? != SaveStatus::Saved
                {
                    return Err(DeskActError(
                        "Save As remained open after one observed retry".into(),
                    ));
                }
            }
        }
    }

    let saved_window = unique_window_for_id(cli, &original)?;
    let saved_title = format!("{filename} - Notepad");
    if !saved_window.title.eq_ignore_ascii_case(&saved_title) {
        return Err(DeskActError(format!(
            "Saved test-tab identity was not confirmed by its exact unique filename (expected `{saved_title}`, observed `{}`). The tab was not closed.",
            saved_window.title
        )));
    }
    let saved_snapshot = fresh_snapshot(cli, &saved_window)?;
    let editor = unique_control(&saved_snapshot, "textfield", None, "saved editor")?;
    let editor_ref = control_ref(editor)?;
    let editor_value = invoke_value(cli, &["get", editor_ref, "--property", "value"], "get")?;
    if editor_value.as_str() != Some(marker.as_str()) {
        return Err(DeskActError(
            "Fresh accessibility readback did not exactly match the fixed marker".into(),
        ));
    }

    press_shortcut(cli, &saved_window, "ctrl+w")?;
    let after_close = wait_for_window_title(cli, &original, &original.title, &saved_title)?;
    if after_close.title != original.title {
        return Err(DeskActError(format!(
            "After closing the verified test tab, Notepad did not return to the original window title `{}` (observed `{}`).",
            original.title, after_close.title
        )));
    }
    artifacts.cleanup()?;
    *ui_may_have_changed = false;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveStatus {
    Saved,
    DialogStillOpen,
}

fn wait_for_saved_contents(
    cli: &VerifiedNativeCli,
    original: &NativeWindow,
    dialog: &NativeWindow,
    path: &Path,
    marker: &str,
    timeout: Duration,
) -> Result<SaveStatus, DeskActError> {
    let deadline = Instant::now() + timeout;
    loop {
        let file_exists = match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => true,
            Ok(_) => {
                return Err(DeskActError(
                    "The acceptance output path is not a regular file".into(),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => {
                return Err(DeskActError(format!(
                    "Could not inspect the unique acceptance output: {error}"
                )));
            }
        };
        let contents_match = if file_exists {
            match fs::read_to_string(path) {
                Ok(contents) => contents == marker,
                Err(error) => {
                    return Err(DeskActError(format!(
                        "Saved acceptance file could not be read independently: {error}"
                    )));
                }
            }
        } else {
            false
        };
        let windows = cli.windows_for_app("notepad")?;
        let original_present = windows.iter().any(|window| {
            window.id == original.id
                && window.accessible
                && !window.minimized
                && window.pid == original.pid
        });
        let dialog_present = windows.iter().any(|window| {
            window.id == dialog.id
                && window.title.eq_ignore_ascii_case("Save as")
                && window.accessible
                && !window.minimized
                && window.pid == original.pid
        });
        if !original_present
            || (windows.len() == 1 && dialog_present)
            || (windows.len() != 1 && windows.len() != 2)
            || (windows.len() == 2 && !dialog_present)
        {
            return Err(DeskActError(
                "Save As left an unexpected Notepad window state; no retry was attempted".into(),
            ));
        }
        if contents_match && windows.len() == 1 {
            return Ok(SaveStatus::Saved);
        }
        if Instant::now() >= deadline {
            if dialog_present && (!file_exists || contents_match) {
                return Ok(SaveStatus::DialogStillOpen);
            }
            return Err(if file_exists {
                DeskActError(
                    "Saved acceptance file contents did not exactly match the fixed marker".into(),
                )
            } else {
                DeskActError(
                    "Save As action did not produce the unique acceptance file before the verification deadline".into(),
                )
            });
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn press_shortcut(
    cli: &VerifiedNativeCli,
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
    invoke_success(cli, &args, "press")?;
    Ok(())
}

fn wait_for_window_title(
    cli: &VerifiedNativeCli,
    original: &NativeWindow,
    expected_title: &str,
    pending_title: &str,
) -> Result<NativeWindow, DeskActError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let windows = cli.windows_for_app("notepad")?;
        if windows.len() != 1 {
            return Err(DeskActError(format!(
                "Expected one Notepad window while waiting for `{expected_title}`; found {}",
                windows.len()
            )));
        }
        let window = windows.into_iter().next().expect("one window checked");
        if window.id != original.id
            || window.pid != original.pid
            || !window.accessible
            || window.minimized
        {
            return Err(DeskActError(
                "Notepad window identity changed while waiting for a tab transition".into(),
            ));
        }
        if window.title.eq_ignore_ascii_case(expected_title) {
            return Ok(window);
        }
        if !window.title.eq_ignore_ascii_case(pending_title) {
            return Err(DeskActError(format!(
                "Notepad tab transition was not confirmed: expected `{expected_title}` or pending `{pending_title}`, observed `{}`",
                window.title
            )));
        }
        if Instant::now() >= deadline {
            return Err(DeskActError(format!(
                "Notepad tab did not transition to `{expected_title}` before the deadline"
            )));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn unique_window_for_id(
    cli: &VerifiedNativeCli,
    original: &NativeWindow,
) -> Result<NativeWindow, DeskActError> {
    let windows = cli.windows_for_app("notepad")?;
    if windows.len() != 1 {
        return Err(DeskActError(format!(
            "Expected the selected Notepad window to remain unique; found {} windows",
            windows.len()
        )));
    }
    let matching: Vec<_> = windows
        .into_iter()
        .filter(|window| window.id == original.id)
        .collect();
    match matching.as_slice() {
        [window] if window.accessible && !window.minimized && window.pid == original.pid => {
            Ok(window.clone())
        }
        [] => Err(DeskActError(format!(
            "The selected Notepad window `{}` disappeared",
            original.id
        ))),
        _ => Err(DeskActError(format!(
            "The selected Notepad window `{}` became ambiguous or unsafe",
            original.id
        ))),
    }
}

fn wait_for_save_dialog(
    cli: &VerifiedNativeCli,
    original: &NativeWindow,
) -> Result<NativeWindow, DeskActError> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let windows = cli.windows_for_app("notepad")?;
        if windows.len() == 2 {
            let original_present = windows.iter().any(|window| {
                window.id == original.id
                    && window.accessible
                    && !window.minimized
                    && window.pid == original.pid
            });
            if !original_present {
                return Err(DeskActError(
                    "The original Notepad window changed while Save As appeared".into(),
                ));
            }
            let matching: Vec<_> = windows
                .into_iter()
                .filter(|window| {
                    window.id != original.id
                        && window.title.eq_ignore_ascii_case("Save as")
                        && window.accessible
                        && !window.minimized
                        && window.pid == original.pid
                })
                .collect();
            return match matching.as_slice() {
                [window] => Ok(window.clone()),
                _ => Err(DeskActError(format!(
                    "Expected one accessible Save As dialog owned by Notepad; found {}",
                    matching.len()
                ))),
            };
        }
        if windows.len() != 1
            || windows[0].id != original.id
            || windows[0].pid != original.pid
            || !windows[0].accessible
            || windows[0].minimized
        {
            return Err(DeskActError(
                "Unexpected Notepad window state while waiting for Save As".into(),
            ));
        }
        if Instant::now() >= deadline {
            return Err(DeskActError(
                "Save As dialog did not appear before the readiness deadline".into(),
            ));
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn fresh_snapshot(
    cli: &VerifiedNativeCli,
    window: &NativeWindow,
) -> Result<serde_json::Value, DeskActError> {
    let output = cli.invoke_args(
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

fn unique_control<'a>(
    snapshot: &'a serde_json::Value,
    role: &str,
    name: Option<&str>,
    description: &str,
) -> Result<&'a serde_json::Value, DeskActError> {
    fn visit<'a>(
        node: &'a serde_json::Value,
        role: &str,
        name: Option<&str>,
        matches: &mut Vec<&'a serde_json::Value>,
    ) {
        if node.get("role").and_then(serde_json::Value::as_str) == Some(role)
            && name.is_none_or(|expected| {
                node.get("name").and_then(serde_json::Value::as_str) == Some(expected)
            })
            && node
                .get("ref_id")
                .and_then(serde_json::Value::as_str)
                .is_some()
        {
            matches.push(node);
        }
        if let Some(children) = node.get("children").and_then(serde_json::Value::as_array) {
            for child in children {
                visit(child, role, name, matches);
            }
        }
    }
    let mut matches = Vec::new();
    if let Some(tree) = snapshot.get("tree") {
        visit(tree, role, name, &mut matches);
    }
    match matches.as_slice() {
        [control] => Ok(*control),
        _ => Err(DeskActError(format!(
            "Expected one unique {description}; found {}",
            matches.len()
        ))),
    }
}

fn control_ref(control: &serde_json::Value) -> Result<&str, DeskActError> {
    control
        .get("ref_id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| DeskActError("Accessibility control had no fresh ref".into()))
}

fn invoke_success(
    cli: &VerifiedNativeCli,
    args: &[&str],
    command: &str,
) -> Result<serde_json::Value, DeskActError> {
    let output = cli.invoke_args(args, command)?;
    successful_data(output, command)
}

fn invoke_value(
    cli: &VerifiedNativeCli,
    args: &[&str],
    command: &str,
) -> Result<serde_json::Value, DeskActError> {
    let data = invoke_success(cli, args, command)?;
    data.get("value").cloned().ok_or_else(|| {
        DeskActError(format!(
            "agent-desktop `{command}` result did not include a value"
        ))
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

/// Launch a uniquely named discovered app and resolve its fresh accessible window.
pub fn resolve_native_target(app_name: &str) -> Result<NativeTargetResolution, DeskActError> {
    let cli = VerifiedNativeCli::connect()?;
    let discovery = windows_catalog::discover_app_catalog()?;
    let catalog = normalize_catalog(discovery.entries);
    let matches: Vec<_> = catalog
        .into_iter()
        .filter(|app| app.name.to_lowercase() == app_name.trim().to_lowercase())
        .collect();
    let app = match matches.as_slice() {
        [] => {
            return Err(DeskActError(format!(
                "No discovered app exactly matches `{}`. Choose a name from `deskact-dev inventory`.",
                app_name.trim()
            )));
        }
        [app] => app.clone(),
        _ => {
            return Err(DeskActError(format!(
                "Multiple discovered entries are named `{}`. No app was launched.",
                app_name.trim()
            )));
        }
    };
    cli.resolve_target(app)
}

/// Sort and remove only catalog entries with identical launch configuration.
fn normalize_catalog(mut catalog: Vec<DiscoveredApp>) -> Vec<DiscoveredApp> {
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

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn subprocess_deadline_terminates_and_reaps_child() {
        let mut command = Command::new("sh");
        command.args(["-c", "exec sleep 10"]);
        let started = Instant::now();

        let result = run_command_with_timeout(&mut command, Duration::from_millis(50));

        assert!(matches!(result, Err(CommandRunError::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn notepad_acceptance_uses_a_ninety_five_percent_success_gate() {
        assert_eq!(required_successes(20), 19);
        assert_eq!(required_successes(1), 1);
        assert_eq!(required_successes(100), 95);
        assert_eq!(
            required_successes(usize::MAX),
            (usize::MAX / 100) * 95 + (usize::MAX % 100 * 95).div_ceil(100)
        );
    }
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
