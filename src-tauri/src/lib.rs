//! Read-only native CLI preflight and Windows application inventory for DeskAct.

use std::{
    env,
    ffi::{OsStr, OsString},
    fmt,
    io::{self, Read},
    process::{Child, Command, Output, Stdio},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::Deserialize;

mod windows_catalog;
pub use windows_catalog::{CatalogDiscovery, DiscoveredApp, DiscoverySource, discover_app_catalog};

/// Exact `agent-desktop` release required for native CLI operations.
pub const SUPPORTED_AGENT_DESKTOP_VERSION: &str = "1.1.0";
/// Structured JSON contract version required from `agent-desktop`.
pub const SUPPORTED_JSON_CONTRACT_VERSION: &str = "2.6";
const INSTALL_GUIDANCE: &str = "Install the required version from Windows PowerShell with `npm install --global agent-desktop@1.1.0`; DeskAct never installs or updates automation tools.";
pub(crate) const NATIVE_PROCESS_TIMEOUT: Duration = Duration::from_secs(30);

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
    pub id: String,
    pub title: String,
    pub app_name: String,
    pub accessible: bool,
    pub minimized: bool,
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub pid: u32,
}

/// Resolution may pause so the user can manually restore a minimized window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeTargetResolution {
    Ready(ResolvedNativeTarget),
    NeedsManualRestore {
        app: DiscoveredApp,
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

    fn invoke_args(&self, args: &[&str], command_name: &str) -> Result<Output, DeskActError> {
        let mut command = Command::new(&self.executable);
        command.args(args);
        match run_command_with_timeout(&mut command, NATIVE_PROCESS_TIMEOUT) {
            Ok(output) => Ok(output),
            Err(CommandRunError::TimedOut) => Err(DeskActError(format!(
                "agent-desktop `{command_name}` timed out after {} seconds",
                NATIVE_PROCESS_TIMEOUT.as_secs()
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
        self.resolve_window_state(app)
    }

    /// Resume after the user restores a minimized app; never relaunches it.
    pub fn resume_after_manual_restore(
        &self,
        app: DiscoveredApp,
    ) -> Result<NativeTargetResolution, DeskActError> {
        self.resolve_window_state(app)
    }

    fn resolve_window_state(
        &self,
        app: DiscoveredApp,
    ) -> Result<NativeTargetResolution, DeskActError> {
        let output = self.invoke("list-windows")?;
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
        let expected_process = executable_stem(&app.target);
        let matching: Vec<_> = windows
            .into_iter()
            .filter(|window| window.app_name.eq_ignore_ascii_case(&expected_process))
            .collect();
        if matching.is_empty() {
            return Err(DeskActError(format!(
                "No fresh window matched `{}` (target process `{expected_process}`) after launch. No window was selected.",
                app.name
            )));
        }
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
        if accessible.len() != 1 {
            return Err(DeskActError(format!(
                "Found {} accessible windows matching `{}`; exactly one is required. No window was selected.",
                accessible.len(),
                app.name
            )));
        }
        let window = accessible
            .into_iter()
            .next()
            .expect("one accessible window");
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

/// Launch a uniquely named discovered app and resolve its fresh accessible window.
pub fn resolve_native_target(app_name: &str) -> Result<NativeTargetResolution, DeskActError> {
    let cli = VerifiedNativeCli::connect()?;
    let discovery = windows_catalog::discover_app_catalog()?;
    let catalog = normalize_catalog(discovery.entries);
    let matches: Vec<_> = catalog
        .into_iter()
        .filter(|app| app.name.eq_ignore_ascii_case(app_name.trim()))
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

fn normalize_catalog(mut catalog: Vec<DiscoveredApp>) -> Vec<DiscoveredApp> {
    catalog.sort_by_key(|app| {
        (
            app.name.to_lowercase(),
            app.target.to_lowercase(),
            app.arguments.clone(),
        )
    });
    catalog.dedup_by(|left, right| {
        left.name.eq_ignore_ascii_case(&right.name)
            && left.target.eq_ignore_ascii_case(&right.target)
            && left.arguments == right.arguments
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
