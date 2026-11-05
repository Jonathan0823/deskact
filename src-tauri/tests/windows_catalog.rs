#![cfg(windows)]

use std::{
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use deskact_core::{DiscoverySource, discover_app_catalog, process_image_path};

struct WindowsCatalogFixture {
    name: String,
}

impl WindowsCatalogFixture {
    fn create() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after Unix epoch")
            .as_nanos();
        let fixture = Self {
            name: format!("DeskActFixture-{}-{nonce}", std::process::id()),
        };
        let script = format!(
            r#"
$ErrorActionPreference = 'Stop'
$name = '{name}'
$startMenu = [Environment]::GetFolderPath([System.Environment+SpecialFolder]::StartMenu)
$fixtureDir = Join-Path $startMenu $name
New-Item -ItemType Directory -Path $fixtureDir -Force | Out-Null
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut((Join-Path $fixtureDir "$name.lnk"))
$shortcut.TargetPath = "$env:SystemRoot\System32\notepad.exe"
$shortcut.Save()
$emptyShortcut = $shell.CreateShortcut((Join-Path $fixtureDir "$name-empty.lnk"))
$emptyShortcut.Description = 'DeskAct no-file-target test fixture'
$emptyShortcut.Save()
$key = "Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\App Paths\$name.exe"
New-Item -Path $key -Force | Out-Null
Set-Item -LiteralPath $key -Value "$env:SystemRoot\System32\calc.exe"
"#,
            name = fixture.name
        );
        let output = run_powershell(&script);
        assert_success(&output, "create Windows catalog fixtures");
        fixture
    }
}

impl Drop for WindowsCatalogFixture {
    fn drop(&mut self) {
        let script = format!(
            r#"
$name = '{name}'
$startMenu = [Environment]::GetFolderPath([System.Environment+SpecialFolder]::StartMenu)
$fixtureDir = Join-Path $startMenu $name
Remove-Item -LiteralPath $fixtureDir -Recurse -Force -ErrorAction SilentlyContinue
$key = "Registry::HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\App Paths\$name.exe"
Remove-Item -LiteralPath $key -Recurse -Force -ErrorAction SilentlyContinue
"#,
            name = self.name
        );
        let _ = run_powershell(&script);
    }
}

fn run_powershell(script: &str) -> Output {
    Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .expect("start Windows PowerShell")
}

fn assert_success(output: &Output, action: &str) {
    assert!(
        output.status.success(),
        "Could not {action}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn powershell_reports_the_current_process_image_path() {
    let expected = std::env::current_exe().expect("get current test executable");
    let actual = process_image_path(std::process::id()).expect("query current process image path");
    let expected_name = expected.file_name().expect("executable has a filename");
    let actual_name = std::path::Path::new(&actual)
        .file_name()
        .expect("reported image path has a filename");

    assert!(expected_name.eq_ignore_ascii_case(actual_name));
}

#[test]
fn real_powershell_discovers_isolated_start_menu_and_app_paths_fixtures() {
    let fixture = WindowsCatalogFixture::create();

    let discovery = discover_app_catalog().expect("discover Windows app catalog");

    assert!(
        discovery.entries.iter().any(|entry| {
            entry.name == fixture.name && entry.source == DiscoverySource::StartMenu
        }),
        "Start Menu fixture was not found"
    );
    assert!(
        discovery.entries.iter().any(|entry| {
            entry.name == fixture.name && entry.source == DiscoverySource::AppPaths
        }),
        "App Paths fixture was not found"
    );
    assert!(
        discovery.skipped_start_menu_shortcuts > 0,
        "empty-target shortcut was not counted as skipped"
    );
}
