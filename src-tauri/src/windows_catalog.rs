use std::{env, fmt, process::Command};

use serde::Deserialize;

use crate::{CommandRunError, DeskActError, NATIVE_PROCESS_TIMEOUT, run_command_with_timeout};

/// An inspected Start Menu shortcut or App Paths registration; its target is not launch authorization.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DiscoveredApp {
    /// Display name from the shortcut or registry entry.
    pub name: String,
    /// Target value as reported by Windows; it is not a launch authorization.
    pub target: String,
    /// Shortcut arguments, preserved without normalization.
    pub arguments: String,
    /// Shortcut working directory, when present.
    pub working_directory: String,
    /// Windows source from which the entry was read.
    pub source: DiscoverySource,
}

/// Results from inspecting Windows app-discovery sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogDiscovery {
    /// Discovered entries, before the caller applies deterministic ordering/deduplication.
    pub entries: Vec<DiscoveredApp>,
    /// Start Menu shortcuts that had no file target and were therefore skipped.
    pub skipped_start_menu_shortcuts: usize,
}

#[derive(Deserialize)]
struct DiscoveryResponse {
    entries: Vec<DiscoveredApp>,
    skipped_start_menu_shortcuts: usize,
}

/// Origin of a discovered Windows application entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiscoverySource {
    /// Entry discovered from a user or common Start Menu shortcut.
    StartMenu,
    /// Entry discovered from a registered Windows App Paths key.
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

const APP_DISCOVERY_SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object -TypeName System.Text.UTF8Encoding -ArgumentList $false
$OutputEncoding = [Console]::OutputEncoding
try {
    $entries = New-Object 'System.Collections.Generic.List[object]'
    $shell = New-Object -ComObject WScript.Shell
    $skippedStartMenuShortcuts = 0
    $startMenuRoots = @(
        [Environment]::GetFolderPath([System.Environment+SpecialFolder]::StartMenu),
        [Environment]::GetFolderPath([System.Environment+SpecialFolder]::CommonStartMenu)
    ) | Where-Object { $_ }
    foreach ($root in $startMenuRoots) {
        try {
            [void][System.IO.File]::GetAttributes($root)
        } catch [System.IO.DirectoryNotFoundException] {
            $parent = [System.IO.Directory]::GetParent($root)
            if ($null -eq $parent) { throw }
            [void][System.IO.File]::GetAttributes($parent.FullName)
            continue
        }
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
            } else {
                $skippedStartMenuShortcuts++
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
    $result = [PSCustomObject]@{
        entries = $entries.ToArray()
        skipped_start_menu_shortcuts = $skippedStartMenuShortcuts
    }
    ConvertTo-Json -InputObject $result -Depth 4 -Compress
} catch {
    [Console]::Error.WriteLine($_.Exception.Message)
    exit 1
}
"#;

/// Discover Start Menu shortcuts and registered App Paths entries with Windows PowerShell.
pub fn discover_app_catalog() -> Result<CatalogDiscovery, DeskActError> {
    let powershell = env::var_os("DESKACT_POWERSHELL").unwrap_or_else(|| "powershell.exe".into());
    let mut command = Command::new(&powershell);
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        APP_DISCOVERY_SCRIPT,
    ]);
    let output = match run_command_with_timeout(&mut command, NATIVE_PROCESS_TIMEOUT) {
        Ok(output) => output,
        Err(CommandRunError::TimedOut) => {
            return Err(DeskActError::new(format!(
                "Windows app discovery timed out after {} seconds",
                NATIVE_PROCESS_TIMEOUT.as_secs()
            )));
        }
        Err(CommandRunError::Io(error)) => {
            let executable = powershell.to_string_lossy();
            return Err(DeskActError::new(format!(
                "Could not run Windows app discovery ({executable}): {error}"
            )));
        }
    };

    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(DeskActError::new(if details.is_empty() {
            format!("Windows app discovery exited with status {}", output.status)
        } else {
            format!("Windows app discovery failed: {details}")
        }));
    }

    let response: DiscoveryResponse = serde_json::from_slice(&output.stdout).map_err(|error| {
        DeskActError::new(format!(
            "Windows app discovery returned invalid JSON: {error}"
        ))
    })?;
    Ok(CatalogDiscovery {
        entries: response.entries,
        skipped_start_menu_shortcuts: response.skipped_start_menu_shortcuts,
    })
}

/// Read the current executable image path for a window owner PID.
///
/// The path is obtained from a fresh Windows process query, not inferred from the app label or launch result.
/// If the OS cannot provide it (including hosted-app/container cases), resolution fails closed.
pub fn process_image_path(pid: u32) -> Result<String, DeskActError> {
    let powershell = env::var_os("DESKACT_POWERSHELL").unwrap_or_else(|| "powershell.exe".into());
    let script = format!(
        "$process = Get-CimInstance -ClassName Win32_Process -Filter 'ProcessId = {pid}' -ErrorAction Stop; if ($null -eq $process -or [string]::IsNullOrWhiteSpace($process.ExecutablePath)) {{ exit 2 }}; [Console]::WriteLine($process.ExecutablePath)"
    );
    let mut command = Command::new(&powershell);
    command.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    let output = match run_command_with_timeout(&mut command, NATIVE_PROCESS_TIMEOUT) {
        Ok(output) => output,
        Err(CommandRunError::TimedOut) => {
            return Err(DeskActError::new(format!(
                "Could not determine the executable path for window owner PID {pid} before the process-query deadline. The app may be hosted; no window was selected."
            )));
        }
        Err(CommandRunError::Io(error)) => {
            return Err(DeskActError::new(format!(
                "Could not query the executable path for window owner PID {pid}: {error}. No window was selected."
            )));
        }
    };
    if !output.status.success() {
        return Err(DeskActError::new(format!(
            "Could not determine the executable path for window owner PID {pid}. The app may be hosted by a container process; no window was selected."
        )));
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if path.is_empty() {
        return Err(DeskActError::new(format!(
            "Windows returned no executable path for window owner PID {pid}. The app may be hosted; no window was selected."
        )));
    }
    Ok(path)
}
