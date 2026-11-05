use std::{env, fmt, process::Command};

use serde::Deserialize;

use crate::DeskActError;

/// An inspected Start Menu shortcut or App Paths registration; its target is not launch authorization.
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

const APP_DISCOVERY_SCRIPT: &str = r#"
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

pub fn discover_app_catalog() -> Result<Vec<DiscoveredApp>, DeskActError> {
    let powershell = env::var_os("DESKACT_POWERSHELL").unwrap_or_else(|| "powershell.exe".into());
    let output = Command::new(&powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            APP_DISCOVERY_SCRIPT,
        ])
        .output()
        .map_err(|error| {
            let executable = powershell.to_string_lossy();
            DeskActError::new(format!(
                "Could not run Windows app discovery ({executable}): {error}"
            ))
        })?;

    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(DeskActError::new(if details.is_empty() {
            format!("Windows app discovery exited with status {}", output.status)
        } else {
            format!("Windows app discovery failed: {details}")
        }));
    }

    serde_json::from_slice(&output.stdout).map_err(|error| {
        DeskActError::new(format!(
            "Windows app discovery returned invalid JSON: {error}"
        ))
    })
}
