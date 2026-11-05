use std::{
    env,
    ffi::OsString,
    fmt,
    process::{Command, Output},
};

use serde::Deserialize;

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

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DiscoveredApp {
    pub name: String,
    pub target: String,
    pub arguments: String,
    pub working_directory: String,
    pub source: DiscoverySource,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoverySource {
    StartMenu,
    AppPaths,
}

impl fmt::Display for DiscoverySource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StartMenu => "start-menu",
            Self::AppPaths => "app-paths",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskActError(String);

impl fmt::Display for DeskActError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DeskActError {}

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
}

#[derive(Deserialize)]
struct AppInventory {
    apps: Vec<RunningApp>,
}

const WINDOWS_APP_DISCOVERY_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object -TypeName System.Text.UTF8Encoding -ArgumentList $false
$OutputEncoding = [Console]::OutputEncoding
try {
    $entries = New-Object 'System.Collections.Generic.List[object]'
    $shell = New-Object -ComObject WScript.Shell
    $startMenuRoots = @(
        [Environment]::GetFolderPath([System.Environment+SpecialFolder]::StartMenu),
        [Environment]::GetFolderPath([System.Environment+SpecialFolder]::CommonStartMenu)
    ) | Where-Object { $_ -and [System.IO.Directory]::Exists($_) }
    foreach ($root in $startMenuRoots) {
        Get-ChildItem -LiteralPath $root -Filter '*.lnk' -File -Recurse -ErrorAction Stop | ForEach-Object {
            $shortcut = $shell.CreateShortcut($_.FullName)
            if ($shortcut.TargetPath) {
                $null = $entries.Add([PSCustomObject]@{
                    name = [IO.Path]::GetFileNameWithoutExtension($_.Name)
                    target = [string]$shortcut.TargetPath
                    arguments = [string]$shortcut.Arguments
                    working_directory = [string]$shortcut.WorkingDirectory
                    source = 'start-menu'
                })
            }
        }
    }
    $registeredRoots = @(
        'Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\App Paths',
        'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths',
        'Registry::HKEY_LOCAL_MACHINE\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths'
    )
    foreach ($root in $registeredRoots) {
        if (Test-Path -LiteralPath $root) {
            Get-ChildItem -LiteralPath $root -ErrorAction Stop | ForEach-Object {
                $key = Get-Item -LiteralPath $_.PSPath -ErrorAction Stop
                $target = [string]$key.GetValue('')
                if ($target) {
                    $null = $entries.Add([PSCustomObject]@{
                        name = [IO.Path]::GetFileNameWithoutExtension($_.PSChildName)
                        target = $target
                        arguments = ''
                        working_directory = ''
                        source = 'app-paths'
                    })
                }
            }
        }
    }
    $sorted = @($entries | Sort-Object name, target, arguments, source -Unique)
    ConvertTo-Json -InputObject $sorted -Depth 4 -Compress
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
"#;

/// Verify the installed native automation CLI and its structured-output contract.
pub fn preflight() -> Result<PreflightStatus, DeskActError> {
    let output = invoke_agent_desktop("version")?;
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

    Ok(PreflightStatus {
        tool_version: data.version,
        contract_version: envelope.version,
    })
}

/// Return the currently running GUI applications after verifying tool compatibility.
pub fn native_inventory() -> Result<NativeInventory, DeskActError> {
    let preflight = preflight()?;
    let output = invoke_agent_desktop("list-apps")?;
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

    if envelope.command != "list-apps" {
        return Err(DeskActError(format!(
            "agent-desktop returned command `{}` for a `list-apps` request",
            envelope.command
        )));
    }

    let mut running_apps = envelope
        .data
        .ok_or_else(|| DeskActError("agent-desktop app inventory did not include data".into()))?
        .apps;
    running_apps.sort_by_key(|app| app.name.to_lowercase());

    let mut catalog = discover_app_catalog()?;
    catalog.retain(|app| is_executable_target(&app.target));
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

fn is_executable_target(target: &str) -> bool {
    let file_name = target.rsplit(['\\', '/']).next().unwrap_or(target);
    let Some((_, extension)) = file_name.rsplit_once('.') else {
        return false;
    };
    extension.eq_ignore_ascii_case("exe") || extension.eq_ignore_ascii_case("com")
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

fn discover_app_catalog() -> Result<Vec<DiscoveredApp>, DeskActError> {
    let powershell =
        env::var_os("DESKACT_POWERSHELL").unwrap_or_else(|| OsString::from("powershell.exe"));
    let output = Command::new(&powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            WINDOWS_APP_DISCOVERY_SCRIPT,
        ])
        .output()
        .map_err(|error| {
            let executable = powershell.to_string_lossy();
            DeskActError(format!(
                "Could not run Windows app discovery ({executable}): {error}"
            ))
        })?;

    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(DeskActError(if details.is_empty() {
            format!("Windows app discovery exited with status {}", output.status)
        } else {
            format!("Windows app discovery failed: {details}")
        }));
    }

    serde_json::from_slice(&output.stdout).map_err(|error| {
        DeskActError(format!(
            "Windows app discovery returned invalid JSON: {error}"
        ))
    })
}

fn invoke_agent_desktop(command: &str) -> Result<Output, DeskActError> {
    let executable = configured_executable();
    Command::new(&executable)
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
