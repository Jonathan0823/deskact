use std::{
    env,
    ffi::{OsStr, OsString},
    fmt,
    process::{Command, Output},
};

use serde::Deserialize;

mod windows_catalog;
pub use windows_catalog::{DiscoveredApp, DiscoverySource};

pub const SUPPORTED_AGENT_DESKTOP_VERSION: &str = "1.1.0";
pub const SUPPORTED_JSON_CONTRACT_VERSION: &str = "2.6";
const INSTALL_GUIDANCE: &str = "Install the required version from Windows PowerShell with `npm install --global agent-desktop@1.1.0`; DeskAct never installs or updates automation tools.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightStatus {
    pub tool_version: String,
    pub contract_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunningApp {
    pub name: String,
    pub pid: u32,
    pub presentation: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeInventory {
    pub preflight: PreflightStatus,
    pub catalog: Vec<DiscoveredApp>,
    pub running_apps: Vec<RunningApp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskActError(String);

impl DeskActError {
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
    pub fn connect() -> Result<Self, DeskActError> {
        let executable = configured_executable();
        let output = run_agent_desktop(&executable, "version")?;
        let envelope: Envelope<VersionData> = parse_envelope(&output, "version")?;

        if envelope.version != SUPPORTED_JSON_CONTRACT_VERSION {
            return Err(DeskActError(format!(
                "Unsupported agent-desktop JSON contract {} (DeskAct requires {}). {INSTALL_GUIDANCE}",
                envelope.version, SUPPORTED_JSON_CONTRACT_VERSION
            )));
        }
        if !envelope.ok {
            return Err(cli_error("version", envelope.error));
        }
        let data = envelope.data.ok_or_else(|| {
            DeskActError("agent-desktop version response did not include data".into())
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

    pub fn preflight_status(&self) -> &PreflightStatus {
        &self.preflight
    }

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
    let mut catalog = windows_catalog::discover_app_catalog()?;
    catalog.sort_by_key(|app| {
        (
            app.name.to_lowercase(),
            app.target.to_lowercase(),
            app.arguments.to_lowercase(),
        )
    });
    catalog.dedup_by(|left, right| {
        left.name.eq_ignore_ascii_case(&right.name)
            && left.target.eq_ignore_ascii_case(&right.target)
            && left.arguments == right.arguments
    });

    Ok(NativeInventory {
        preflight,
        catalog,
        running_apps,
    })
}

fn parse_envelope<T: for<'de> Deserialize<'de>>(
    output: &Output,
    requested_command: &str,
) -> Result<Envelope<T>, DeskActError> {
    if !output.status.success() {
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
    Command::new(executable)
        .arg(command)
        .output()
        .map_err(|error| {
            let executable = executable.to_string_lossy();
            if error.kind() == std::io::ErrorKind::NotFound {
                DeskActError(format!(
                    "Could not find or start agent-desktop ({executable}). {INSTALL_GUIDANCE}"
                ))
            } else {
                DeskActError(format!(
                    "Could not start agent-desktop ({executable}): {error}. {INSTALL_GUIDANCE}"
                ))
            }
        })
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
