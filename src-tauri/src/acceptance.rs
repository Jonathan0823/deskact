//! Controlled developer acceptance workflows that exercise File Explorer, Calculator, VLC, and Notepad through the verified native session.

use std::{
    cell::Cell,
    collections::HashSet,
    env,
    ffi::OsStr,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use crate::{
    APP_READINESS_TIMEOUT, DeskActError, NativeWindow, VerifiedNativeCli,
    native::{
        CommandRunError, NATIVE_PROCESS_TIMEOUT, PowerShellRun, executable_stem, normalize_catalog,
        poll_until_deadline, run_command_with_timeout, run_powershell, windows_paths_equal,
    },
    process_image_path, snapshot, windows_catalog,
};

/// Create and open a unique text fixture through File Explorer's accessible UI, then verify fresh UI evidence.
pub fn run_file_explorer_acceptance() -> Result<(), DeskActError> {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let directory = env::temp_dir().join(format!("deskact-explorer-{unique}"));
    let marker = format!("DESKACT_FILE_EXPLORER_FIXTURE_{unique}");
    fs::create_dir(&directory).map_err(|error| {
        DeskActError(format!(
            "Could not create unique Explorer fixture folder: {error}"
        ))
    })?;
    let file = directory.join(format!("deskact-fixture-{unique}.txt"));
    let result = (|| {
        fs::write(&file, &marker).map_err(|error| {
            DeskActError(format!("Could not create Explorer fixture file: {error}"))
        })?;
        let cli = VerifiedNativeCli::connect()?;
        let mut windows = cli.windows_for_app("explorer")?;
        if windows.is_empty() {
            // Explorer's shell process launch may not expose a window to agent-desktop. This fallback only opens Explorer; it never supplies a fixture path or performs navigation.
            let launched = run_powershell(PowerShellRun {
                script: "Start-Process explorer.exe",
                non_interactive: false,
                capture_output: false,
                envs: &[],
                action: "launch File Explorer",
                timeout_message: format!(
                    "PowerShell timed out while launching File Explorer after {} seconds",
                    NATIVE_PROCESS_TIMEOUT.as_secs()
                ),
            })?;
            if !launched.status.success() {
                return Err(DeskActError(
                    "PowerShell Start-Process could not launch File Explorer".into(),
                ));
            }
            windows = poll_until_deadline(
                Instant::now() + APP_READINESS_TIMEOUT,
                |remaining| {
                    let windows = cli.windows_for_app_with_timeout("explorer", remaining)?;
                    Ok((!windows.is_empty()).then_some(windows))
                },
                || Ok(Vec::new()),
            )?;
        }
        if windows.len() != 1 {
            return Err(DeskActError(format!(
                "Expected exactly one accessible Explorer window; found {}. No window was selected.",
                windows.len()
            )));
        }
        let window = &windows[0];
        if !window.accessible || window.minimized || !window.visible || window.pid == 0 {
            return Err(DeskActError(
                "The unique Explorer window was not visible, accessible, and verifiably identified"
                    .into(),
            ));
        }
        let owner_path = process_image_path(window.pid)?;
        let windows_directory = env::var_os("WINDIR").ok_or_else(|| {
            DeskActError("WINDIR is unavailable; Explorer ownership cannot be verified".into())
        })?;
        let expected_explorer = Path::new(&windows_directory)
            .join("explorer.exe")
            .to_string_lossy()
            .into_owned();
        if !windows_paths_equal(&owner_path, &expected_explorer) {
            return Err(DeskActError(format!(
                "Explorer window `{}` is not owned by the Windows Explorer executable; no action was taken.",
                window.id
            )));
        }
        let fixture_path = native_ui_path(&directory)?;
        cli.press_app_shortcut(window, &window.app_name, "ctrl+l")?;
        let address_snapshot = cli.fresh_snapshot(window)?;
        let address = snapshot::unique_focused_control(
            &address_snapshot,
            "textfield",
            "Address Bar",
            "Explorer address bar",
        )?;
        cli.invoke_success(
            &["set-value", snapshot::control_ref(address)?, &fixture_path],
            "set-value",
        )?;
        cli.press_app_shortcut(window, &window.app_name, "enter")?;
        let filename = file
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| DeskActError("Fixture filename is not Unicode".into()))?;
        let navigated = wait_for_fixture_in_explorer(&cli, window, filename)?;
        snapshot::unique_control(&navigated, "option", Some(filename), "fixture file")?;
        cli.press_app_shortcut(window, &window.app_name, "ctrl+l")?;
        let location_snapshot = cli.fresh_snapshot(window)?;
        let address = snapshot::unique_focused_control(
            &location_snapshot,
            "textfield",
            "Address Bar",
            "Explorer address bar",
        )?;
        if address
            .get("value")
            .and_then(serde_json::Value::as_str)
            .is_none_or(|observed| !windows_paths_equal(observed, &fixture_path))
        {
            return Err(DeskActError(
                "Fresh Explorer evidence did not confirm the exact fixture folder path".into(),
            ));
        }
        cli.press_app_shortcut(window, &window.app_name, "escape")?;
        let listing = cli.fresh_snapshot(window)?;
        let item = snapshot::unique_control(&listing, "option", Some(filename), "fixture file")?;
        cli.invoke_success(&["click", snapshot::control_ref(item)?], "click")?;
        let selected = cli.fresh_snapshot(window)?;
        let item = snapshot::unique_control(&selected, "option", Some(filename), "fixture file")?;
        if !item
            .get("states")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|states| {
                states
                    .iter()
                    .any(|state| state.as_str() == Some("selected"))
            })
        {
            return Err(DeskActError(
                "Fresh Explorer evidence did not confirm selection of the exact fixture filename"
                    .into(),
            ));
        }
        cli.press_app_shortcut(window, &window.app_name, "enter")?;
        wait_for_opened_file(&cli, filename, &marker)?;
        Ok(())
    })();
    let cleanup_file = match fs::remove_file(&file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DeskActError(format!(
            "Could not remove test-created fixture file: {error}"
        ))),
    };
    let cleanup_dir = match fs::remove_dir(&directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DeskActError(format!(
            "Could not remove test-created fixture folder: {error}"
        ))),
    };
    let cleanup = cleanup_file.and(cleanup_dir);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(operation), Ok(())) => Err(operation),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(operation), Err(cleanup)) => Err(DeskActError(format!(
            "{operation}; additionally, cleanup failed: {cleanup}"
        ))),
    }
}

/// Launch or reuse one Calculator window, enter 7 × 6 through fresh accessible controls, and verify 42.
pub fn run_calculator_acceptance() -> Result<(), DeskActError> {
    let cli = VerifiedNativeCli::connect()?;
    let windows = calculator_windows(&cli, NATIVE_PROCESS_TIMEOUT)?;
    let window = match windows.as_slice() {
        [] => {
            launch_calculator()?;
            wait_for_calculator_window(&cli)?
        }
        [window] => window.clone(),
        _ => {
            return Err(DeskActError(format!(
                "Expected one Calculator window; found {}. No calculation was entered.",
                windows.len()
            )));
        }
    };
    validate_calculator_window(&window)?;

    let mut snapshot = cli.fresh_snapshot(&window)?;
    let display =
        calculator_accessible_text(&snapshot, "CalculatorResults", "Display is ", "display")?
            .ok_or_else(|| {
                DeskActError("Calculator's fresh snapshot did not expose its display".into())
            })?;
    let expression = calculator_accessible_text(
        &snapshot,
        "CalculatorExpression",
        "Expression is ",
        "expression",
    )?;
    if display != "0" || expression.as_deref() != Some("") {
        let clear = snapshot::unique_control(
            &snapshot,
            "button",
            Some("Clear"),
            "Calculator Clear button",
        )?;
        if snapshot::has_state(clear, "disabled") {
            return Err(DeskActError(
                "Calculator is not in a clean state and its Clear control is disabled; no calculation was entered".into(),
            ));
        }
        cli.invoke_success(&["click", snapshot::control_ref(clear)?], "click")?;
        snapshot = cli.fresh_snapshot(&window)?;
        let display =
            calculator_accessible_text(&snapshot, "CalculatorResults", "Display is ", "display")?
                .ok_or_else(|| {
                DeskActError(
                    "Calculator's fresh snapshot did not expose its display after Clear".into(),
                )
            })?;
        let expression = calculator_accessible_text(
            &snapshot,
            "CalculatorExpression",
            "Expression is ",
            "expression",
        )?;
        if display != "0" || expression.as_deref().is_some_and(|value| !value.is_empty()) {
            return Err(DeskActError(
                "Fresh Calculator evidence did not confirm the prior calculation was cleared"
                    .into(),
            ));
        }
    }

    for (name, description) in [
        ("Seven", "7 key"),
        ("Multiply by", "multiply key"),
        ("Six", "6 key"),
        ("Equals", "equals key"),
    ] {
        let control = snapshot::unique_control(&snapshot, "button", Some(name), description)?;
        if snapshot::has_state(control, "disabled") {
            return Err(DeskActError(format!(
                "Calculator {description} is disabled; no further input was sent"
            )));
        }
        cli.invoke_success(&["click", snapshot::control_ref(control)?], "click")?;
        snapshot = cli.fresh_snapshot(&window)?;
    }

    let result =
        calculator_accessible_text(&snapshot, "CalculatorResults", "Display is ", "display")?
            .ok_or_else(|| {
                DeskActError("Calculator's final snapshot did not expose its display".into())
            })?;
    if result != "42" {
        return Err(DeskActError(format!(
            "Calculator's fresh accessible display showed `{result}`, not the expected result `42`; the calculation is unverified"
        )));
    }
    Ok(())
}

const CALCULATOR_APP_ID: &str = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";
const CALCULATOR_PROCESS_STEM: &str = "CalculatorApp";

/// Activate packaged Calculator through its AppsFolder app ID with a bounded PowerShell wait.
fn launch_calculator() -> Result<(), DeskActError> {
    let script =
        format!("Start-Process explorer.exe -ArgumentList 'shell:AppsFolder\\{CALCULATOR_APP_ID}'");
    let output = run_powershell(PowerShellRun {
        script: &script,
        non_interactive: true,
        capture_output: false,
        envs: &[],
        action: "activate packaged Calculator",
        timeout_message: format!(
            "PowerShell timed out while activating packaged Calculator after {} seconds",
            NATIVE_PROCESS_TIMEOUT.as_secs()
        ),
    })?;
    if !output.status.success() {
        return Err(DeskActError(
            "PowerShell could not activate the packaged Calculator app".into(),
        ));
    }
    Ok(())
}

/// Return windows whose executable label identifies the packaged Windows Calculator app.
fn calculator_windows(
    cli: &VerifiedNativeCli,
    timeout: Duration,
) -> Result<Vec<NativeWindow>, DeskActError> {
    Ok(cli
        .all_windows_with_timeout(timeout)?
        .into_iter()
        .filter(|window| {
            executable_stem(&window.app_name).eq_ignore_ascii_case(CALCULATOR_PROCESS_STEM)
        })
        .collect())
}

/// Wait for exactly one fresh Calculator window after activation.
fn wait_for_calculator_window(cli: &VerifiedNativeCli) -> Result<NativeWindow, DeskActError> {
    let timed_out = || {
        DeskActError(
            "Calculator did not expose one eligible window before the readiness deadline".into(),
        )
    };
    poll_until_deadline(
        Instant::now() + APP_READINESS_TIMEOUT,
        |remaining| {
            let windows = calculator_windows(cli, remaining)?;
            match windows.as_slice() {
                [window] => Ok(Some(window.clone())),
                [] => Ok(None),
                _ => Err(DeskActError(format!(
                    "Calculator activation exposed {} windows; the target is ambiguous and no calculation was entered",
                    windows.len()
                ))),
            }
        },
        || Err(timed_out()),
    )
}

/// Verify that the selected Calculator window is accessible, visible, and owned by CalculatorApp.exe.
fn validate_calculator_window(window: &NativeWindow) -> Result<(), DeskActError> {
    if !window.accessible || window.minimized || !window.visible || window.pid == 0 {
        return Err(DeskActError(format!(
            "Calculator window `{}` is not visible, accessible, and verifiably identified; no calculation was entered",
            window.id
        )));
    }
    let image_path = process_image_path(window.pid)?;
    if !executable_stem(&image_path).eq_ignore_ascii_case(CALCULATOR_PROCESS_STEM) {
        return Err(DeskActError(format!(
            "Calculator window `{}` is owned by `{image_path}`, not CalculatorApp.exe; no calculation was entered",
            window.id
        )));
    }
    Ok(())
}

/// Read a Calculator UIA text property by automation ID, allowing absent optional expression text.
fn calculator_accessible_text(
    snapshot: &serde_json::Value,
    automation_id: &str,
    prefix: &str,
    description: &str,
) -> Result<Option<String>, DeskActError> {
    let mut matches = snapshot::texts_by_automation_id(snapshot, automation_id, prefix);
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        count => Err(DeskActError(format!(
            "Expected at most one accessible Calculator {description}; found {count}"
        ))),
    }
}

/// Play a generated silent WAV fixture in a dedicated VLC instance and verify advancing UIA playback time.
pub fn run_vlc_acceptance() -> Result<(), DeskActError> {
    let unique = format!(
        "{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let directory = env::temp_dir().join(format!("deskact-vlc-{unique}"));
    fs::create_dir(&directory).map_err(|error| {
        DeskActError(format!(
            "Could not create unique VLC fixture workspace: {error}"
        ))
    })?;
    let media = directory.join(format!("{VLC_FIXTURE_PREFIX}{unique}.wav"));
    let config = directory.join("vlcrc");
    let cli = match VerifiedNativeCli::connect() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = fs::remove_dir_all(&directory);
            return Err(error);
        }
    };
    let mut selected_window = None;
    let mut launched_pid = None;
    let result = (|| {
        write_silent_wav(&media)?;
        let existing_ids: HashSet<String> = cli
            .all_windows_with_timeout(NATIVE_PROCESS_TIMEOUT)?
            .into_iter()
            .filter(|window| executable_stem(&window.app_name).eq_ignore_ascii_case("vlc"))
            .map(|window| window.id)
            .collect();
        let pid = launch_vlc(&media, &config)?;
        launched_pid = Some(pid);
        let fixture_name = media
            .file_name()
            .and_then(OsStr::to_str)
            .ok_or_else(|| DeskActError("VLC fixture filename is not Unicode".into()))?;
        let window = wait_for_new_vlc_window(&cli, &existing_ids, fixture_name, pid)?;
        validate_vlc_window(&window)?;
        selected_window = Some(window.clone());

        let first_snapshot = cli.fresh_snapshot(&window)?;
        require_vlc_playing_state(&first_snapshot)?;
        let (first_position, duration) = vlc_playback_times(&first_snapshot)?;
        if first_position >= duration {
            return Err(DeskActError(
                "VLC exposed no remaining fixture playback time; playback is unverified".into(),
            ));
        }
        thread::sleep(VLC_OBSERVATION_INTERVAL);

        let second_snapshot = cli.fresh_snapshot(&window)?;
        require_vlc_playing_state(&second_snapshot)?;
        let (second_position, second_duration) = vlc_playback_times(&second_snapshot)?;
        if second_duration != duration || second_position >= second_duration {
            return Err(DeskActError(
                "VLC's fresh playback position or duration became inconsistent; outcome is unverified".into(),
            ));
        }
        if second_position <= first_position {
            return Err(DeskActError(format!(
                "VLC playback did not advance between fresh observations ({} then {}); outcome is unverified",
                format_vlc_time(first_position),
                format_vlc_time(second_position)
            )));
        }
        Ok(())
    })();

    let close_result = selected_window
        .as_ref()
        .map_or(Ok(()), |window| close_test_vlc_window(&cli, window));
    let process_cleanup = launched_pid.map_or(Ok(()), |pid| stop_owned_vlc_process(pid, &media));
    let directory_cleanup = match fs::remove_dir_all(&directory) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DeskActError(format!(
            "Could not remove test-created VLC fixture workspace: {error}"
        ))),
    };
    let cleanup = combine_cleanup_results([close_result, process_cleanup, directory_cleanup]);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(operation), Ok(())) => Err(operation),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(operation), Err(cleanup)) => Err(DeskActError(format!(
            "{operation}; additionally, VLC cleanup failed: {cleanup}"
        ))),
    }
}

const VLC_FIXTURE_PREFIX: &str = "deskact-vlc-fixture-";
const VLC_OBSERVATION_INTERVAL: Duration = Duration::from_secs(2);

/// Write a 30-second silent PCM WAV; the fixture cannot expose or contain user media.
fn write_silent_wav(path: &Path) -> Result<(), DeskActError> {
    const SAMPLE_RATE: u32 = 8_000;
    const CHANNELS: u16 = 1;
    const BITS_PER_SAMPLE: u16 = 16;
    const DATA_BYTES: u32 = SAMPLE_RATE * 2 * 30;
    let mut file = fs::File::create(path).map_err(|error| {
        DeskActError(format!("Could not create VLC test media fixture: {error}"))
    })?;
    file.write_all(b"RIFF")
        .and_then(|()| file.write_all(&(36 + DATA_BYTES).to_le_bytes()))
        .and_then(|()| file.write_all(b"WAVEfmt "))
        .and_then(|()| file.write_all(&16_u32.to_le_bytes()))
        .and_then(|()| file.write_all(&1_u16.to_le_bytes()))
        .and_then(|()| file.write_all(&CHANNELS.to_le_bytes()))
        .and_then(|()| file.write_all(&SAMPLE_RATE.to_le_bytes()))
        .and_then(|()| {
            file.write_all(
                &(SAMPLE_RATE * CHANNELS as u32 * BITS_PER_SAMPLE as u32 / 8).to_le_bytes(),
            )
        })
        .and_then(|()| file.write_all(&(CHANNELS * BITS_PER_SAMPLE / 8).to_le_bytes()))
        .and_then(|()| file.write_all(&BITS_PER_SAMPLE.to_le_bytes()))
        .and_then(|()| file.write_all(b"data"))
        .and_then(|()| file.write_all(&DATA_BYTES.to_le_bytes()))
        .and_then(|()| file.write_all(&vec![0; DATA_BYTES as usize]))
        .map_err(|error| DeskActError(format!("Could not write VLC test media fixture: {error}")))
}

/// Launch VLC with an isolated vlcrc, disabled recent-media tracking, and only the generated fixture.
fn launch_vlc(media: &Path, config: &Path) -> Result<u32, DeskActError> {
    let media_path = native_ui_path(media)?;
    let config_path = native_ui_path(config)?;
    let script = r#"
$ErrorActionPreference = 'Stop'
$exe = $env:DESKACT_VLC_EXECUTABLE
if ([string]::IsNullOrWhiteSpace($exe)) {
    $roots = @($env:ProgramFiles, ${env:ProgramFiles(x86)}) | Where-Object { $_ }
    $matches = @($roots | ForEach-Object { Join-Path $_ 'VideoLAN\VLC\vlc.exe' } | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf })
    if ($matches.Count -ne 1) { throw 'Could not identify one VLC executable in the standard VideoLAN install locations' }
    $exe = $matches[0]
}
if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw 'Configured VLC executable does not exist' }
$config = $env:DESKACT_VLC_CONFIG
$media = $env:DESKACT_VLC_MEDIA
$arguments = '--no-one-instance --no-qt-privacy-ask --no-qt-recentplay --no-metadata-network-access --no-qt-updates-notif --config="' + $config + '" "' + $media + '"'
$process = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru
Write-Output "DESKACT_VLC_PID=$($process.Id)"
"#;
    let output = run_powershell(PowerShellRun {
        script,
        non_interactive: true,
        capture_output: true,
        envs: &[
            ("DESKACT_VLC_MEDIA", &media_path),
            ("DESKACT_VLC_CONFIG", &config_path),
        ],
        action: "launch VLC",
        timeout_message: "PowerShell timed out while launching the VLC fixture".into(),
    })?;
    if !output.status.success() {
        return Err(DeskActError(
            "PowerShell could not start VLC with the generated fixture".into(),
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
            line.trim()
                .strip_prefix("DESKACT_VLC_PID=")
                .and_then(|pid| pid.parse::<u32>().ok())
        })
        .ok_or_else(|| DeskActError("PowerShell did not return the launched VLC process ID".into()))
}

/// Stop only the launched VLC PID when its process command line still identifies this fixture.
fn stop_owned_vlc_process(pid: u32, media: &Path) -> Result<(), DeskActError> {
    let media_path = native_ui_path(media)?;
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$pidToStop = {pid}
$media = $env:DESKACT_VLC_MEDIA
$process = Get-CimInstance Win32_Process -Filter "ProcessId=$pidToStop"
if ($null -eq $process) {{ exit 0 }}
if ([IO.Path]::GetFileName($process.ExecutablePath) -ine 'vlc.exe' -or -not $process.CommandLine.Contains($media)) {{
    throw 'Refusing to stop a process that no longer matches the DeskAct VLC fixture'
}}
Stop-Process -Id $pidToStop -Force
"#,
    );
    let output = run_powershell(PowerShellRun {
        script: &script,
        non_interactive: true,
        capture_output: false,
        envs: &[("DESKACT_VLC_MEDIA", &media_path)],
        action: &format!("stop DeskAct-owned VLC process {pid}"),
        timeout_message: format!("Timed out while stopping DeskAct-owned VLC process {pid}"),
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(DeskActError(format!(
            "Could not safely stop DeskAct-owned VLC process {pid}"
        )))
    }
}

/// Collect every cleanup error without skipping later cleanup steps.
fn combine_cleanup_results<const N: usize>(
    results: [Result<(), DeskActError>; N],
) -> Result<(), DeskActError> {
    let errors: Vec<_> = results
        .into_iter()
        .filter_map(Result::err)
        .map(|error| error.0)
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(DeskActError(errors.join("; ")))
    }
}

/// Wait for one newly-created, accessible VLC window associated with the controlled fixture.
fn wait_for_new_vlc_window(
    cli: &VerifiedNativeCli,
    existing_ids: &HashSet<String>,
    fixture_name: &str,
    launched_pid: u32,
) -> Result<NativeWindow, DeskActError> {
    let timed_out = || {
        DeskActError(
            "VLC did not expose the generated fixture in one fresh window before the readiness deadline".into(),
        )
    };
    poll_until_deadline(
        Instant::now() + APP_READINESS_TIMEOUT,
        |remaining| {
            let windows: Vec<_> = cli
                .all_windows_with_timeout(remaining)?
                .into_iter()
                .filter(|window| {
                    executable_stem(&window.app_name).eq_ignore_ascii_case("vlc")
                        && window.pid == launched_pid
                        && window.title.contains(fixture_name)
                        && !existing_ids.contains(&window.id)
                })
                .collect();
            match windows.as_slice() {
                [window] => Ok(Some(window.clone())),
                [] => Ok(None),
                _ => Err(DeskActError(format!(
                    "VLC exposed {} new windows for the fixture; the target is ambiguous",
                    windows.len()
                ))),
            }
        },
        || Err(timed_out()),
    )
}

/// Verify that the selected VLC window is visible, accessible, and owned by vlc.exe.
fn validate_vlc_window(window: &NativeWindow) -> Result<(), DeskActError> {
    if !window.accessible || window.minimized || !window.visible || window.pid == 0 {
        return Err(DeskActError(format!(
            "VLC window `{}` is not visible, accessible, and verifiably identified",
            window.id
        )));
    }
    let image_path = process_image_path(window.pid)?;
    if !executable_stem(&image_path).eq_ignore_ascii_case("vlc") {
        return Err(DeskActError(format!(
            "VLC fixture window `{}` is owned by `{image_path}`, not vlc.exe",
            window.id
        )));
    }
    Ok(())
}

/// Require explicit accessible evidence that VLC is currently playing.
fn require_vlc_playing_state(snapshot: &serde_json::Value) -> Result<(), DeskActError> {
    let states: Vec<bool> = snapshot::nodes(snapshot)
        .filter_map(|node| {
            let role = snapshot::role(node)?;
            let name = snapshot::name(node)?.trim();
            if role == "statictext" && name.eq_ignore_ascii_case("playing") {
                Some(true)
            } else if role == "statictext"
                && (name.eq_ignore_ascii_case("paused") || name.eq_ignore_ascii_case("stopped"))
            {
                Some(false)
            } else if role == "button" && name.eq_ignore_ascii_case("pause") {
                Some(true)
            } else if role == "button" && name.eq_ignore_ascii_case("play") {
                Some(false)
            } else {
                None
            }
        })
        .collect();
    match states.as_slice() {
        [true] => Ok(()),
        [false] => Err(DeskActError(
            "VLC was not in the playing state in a fresh accessibility snapshot; playback is unverified".into(),
        )),
        [] => Err(DeskActError(
            "VLC did not expose an accessible playback state in a fresh snapshot; playback is unverified".into(),
        )),
        _ => Err(DeskActError(
            "VLC exposed conflicting or ambiguous accessible playback state; playback is unverified".into(),
        )),
    }
}

/// Read the current and total playback clock values from exactly two accessible VLC text nodes.
fn vlc_playback_times(snapshot: &serde_json::Value) -> Result<(u64, u64), DeskActError> {
    let times: Vec<u64> = snapshot::nodes(snapshot)
        .filter(|node| snapshot::role(node) == Some("statictext"))
        .filter_map(|node| parse_vlc_time(snapshot::name(node)?))
        .collect();
    match times.as_slice() {
        [position, duration] if *duration > 0 => Ok((*position, *duration)),
        _ => Err(DeskActError(format!(
            "VLC's fresh accessibility snapshot did not expose one current and total playback time pair (found {} time values); playback is unverified",
            times.len()
        ))),
    }
}

/// Parse an accessible VLC time label in MM:SS or HH:MM:SS form.
fn parse_vlc_time(value: &str) -> Option<u64> {
    let parts: Vec<_> = value.trim().split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (
            0,
            minutes.parse::<u64>().ok()?,
            seconds.parse::<u64>().ok()?,
        ),
        [hours, minutes, seconds] => (
            hours.parse::<u64>().ok()?,
            minutes.parse::<u64>().ok()?,
            seconds.parse::<u64>().ok()?,
        ),
        _ => return None,
    };
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    hours
        .checked_mul(3600)?
        .checked_add(minutes.checked_mul(60)?)?
        .checked_add(seconds)
}

/// Format playback time as HH:MM:SS for a clear acceptance report.
fn format_vlc_time(seconds: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// Close only the uniquely verified test VLC window and wait for that identity to disappear.
fn close_test_vlc_window(
    cli: &VerifiedNativeCli,
    window: &NativeWindow,
) -> Result<(), DeskActError> {
    let current = cli.all_windows_with_timeout(NATIVE_PROCESS_TIMEOUT)?;
    if !current
        .iter()
        .any(|candidate| candidate.id == window.id && candidate.pid == window.pid)
    {
        return Ok(());
    }
    validate_vlc_window(window)?;
    cli.press_app_shortcut(window, "vlc.exe", "alt+f4")?;

    poll_until_deadline(
        Instant::now() + APP_READINESS_TIMEOUT,
        |remaining| {
            let windows = cli.all_windows_with_timeout(remaining)?;
            let closed = !windows
                .iter()
                .any(|candidate| candidate.id == window.id && candidate.pid == window.pid);
            Ok(closed.then_some(()))
        },
        || {
            Err(DeskActError(
                "The uniquely selected test VLC window did not close before the cleanup deadline"
                    .into(),
            ))
        },
    )
}

/// Convert a WSL filesystem path to the Windows path Explorer can navigate to.
fn native_ui_path(path: &Path) -> Result<String, DeskActError> {
    let path = path
        .to_str()
        .ok_or_else(|| DeskActError("Fixture path is not Unicode".into()))?;
    #[cfg(target_os = "linux")]
    if env::var_os("WSL_DISTRO_NAME").is_some() {
        let mut command = Command::new("wslpath");
        command.args(["-w", path]);
        let output = match run_command_with_timeout(&mut command, NATIVE_PROCESS_TIMEOUT) {
            Ok(output) => output,
            Err(CommandRunError::TimedOut) => {
                return Err(DeskActError(
                    "wslpath timed out while converting the Explorer fixture path".into(),
                ));
            }
            Err(CommandRunError::Io(error)) => {
                return Err(DeskActError(format!(
                    "Could not convert the WSL fixture path with wslpath: {error}"
                )));
            }
        };
        if !output.status.success() {
            return Err(DeskActError(format!(
                "wslpath could not convert the Explorer fixture path: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let converted = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if converted.is_empty() {
            return Err(DeskActError(
                "wslpath returned an empty Windows fixture path".into(),
            ));
        }
        return Ok(converted);
    }
    Ok(path.to_owned())
}

/// Find the uniquely associated viewer and verify the fixture marker in a fresh snapshot.
fn wait_for_opened_file(
    cli: &VerifiedNativeCli,
    filename: &str,
    marker: &str,
) -> Result<NativeWindow, DeskActError> {
    let deadline = Instant::now() + APP_READINESS_TIMEOUT;
    poll_until_deadline(
        deadline,
        |remaining| {
            let windows = cli.all_windows_with_timeout(remaining)?;
            let title_matches: Vec<_> = windows
                .iter()
                .filter(|window| {
                    !executable_stem(&window.app_name).eq_ignore_ascii_case("explorer")
                        && window.title.contains(filename)
                })
                .cloned()
                .collect();
            if title_matches.len() > 1 {
                return Err(DeskActError(format!(
                    "More than one non-Explorer window matched the unique fixture filename `{filename}`; opened-file evidence is ambiguous."
                )));
            }
            let candidates = if title_matches.is_empty() {
                windows
                    .into_iter()
                    .filter(|window| {
                        !executable_stem(&window.app_name).eq_ignore_ascii_case("explorer")
                            && window.is_focused
                    })
                    .collect()
            } else {
                title_matches
            };
            if candidates.len() > 1 {
                return Err(DeskActError(format!(
                    "More than one non-Explorer window could be the focused viewer for `{filename}`; opened-file evidence is ambiguous."
                )));
            }
            let Some(window) = candidates.into_iter().next() else {
                return Ok(None);
            };
            if !window.accessible || window.minimized || !window.visible || window.pid == 0 {
                return Err(DeskActError(
                    "The fixture's opened-app window was not visible, accessible, and verifiably identified".into(),
                ));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            let snapshot = cli.fresh_snapshot_with_timeout(&window, remaining)?;
            Ok(snapshot::contains_value(&snapshot, marker).then_some(window))
        },
        || {
            Err(DeskActError(format!(
                "The opened fixture file `{filename}` was not confirmed before the readiness deadline"
            )))
        },
    )
}

/// Wait for one accessible Explorer option with the exact fixture filename.
fn wait_for_fixture_in_explorer(
    cli: &VerifiedNativeCli,
    window: &NativeWindow,
    filename: &str,
) -> Result<serde_json::Value, DeskActError> {
    poll_until_deadline(
        Instant::now() + APP_READINESS_TIMEOUT,
        |remaining| {
            let snapshot = cli.fresh_snapshot_with_timeout(window, remaining)?;
            match snapshot::controls(&snapshot, "option", Some(filename)).count() {
                0 => Ok(None),
                1 => Ok(Some(snapshot)),
                count => Err(DeskActError(format!(
                    "Found {count} accessible items named `{filename}` in the fixture folder; no file was opened."
                ))),
            }
        },
        || {
            Err(DeskActError(format!(
                "The fixture file `{filename}` did not appear in a fresh Explorer snapshot before the readiness deadline."
            )))
        },
    )
}

const MAX_NOTEPAD_ACCEPTANCE_RUNS: usize = 100;

/// Run controlled Notepad acceptance scenarios in isolated, newly created tabs.
pub fn run_notepad_acceptance(runs: usize) -> Result<NotepadAcceptanceReport, DeskActError> {
    if runs == 0 {
        return Err(DeskActError(
            "The Notepad acceptance run count must be greater than zero".into(),
        ));
    }
    if runs > MAX_NOTEPAD_ACCEPTANCE_RUNS {
        return Err(DeskActError(format!(
            "Notepad acceptance run count must not exceed {MAX_NOTEPAD_ACCEPTANCE_RUNS}"
        )));
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
        requested: runs,
        attempted,
        verified,
        required,
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
    /// Number of scenarios requested.
    pub requested: usize,
    /// Number of scenarios attempted.
    pub attempted: usize,
    /// Number of scenarios independently verified end to end.
    pub verified: usize,
    /// Minimum verified scenario count required for the requested run count.
    pub required: usize,
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
    cli.press_shortcut(&original, "ctrl+n")?;
    let new_tab = wait_for_window_title(cli, &original, "Untitled - Notepad", &original.title)?;
    if !new_tab.title.eq_ignore_ascii_case("Untitled - Notepad") {
        return Err(DeskActError(format!(
            "Ctrl+N did not produce the expected fresh `Untitled - Notepad` tab (observed `{}`). It was not closed.",
            new_tab.title
        )));
    }
    let editor_snapshot = cli.fresh_snapshot(&new_tab)?;
    let editor = snapshot::unique_control(&editor_snapshot, "textfield", None, "new-tab editor")?;
    let editor_ref = snapshot::control_ref(editor)?;
    let initial_value = cli.invoke_value(&["get", editor_ref, "--property", "value"], "get")?;
    if initial_value.as_str() != Some("") {
        return Err(DeskActError(
            "Fresh Notepad tab was not independently verified empty; no text was changed".into(),
        ));
    }
    cli.invoke_success(&["set-value", editor_ref, &marker], "set-value")?;

    cli.press_shortcut(&new_tab, "ctrl+s")?;
    let dialog = wait_for_save_dialog(cli, &original)?;
    let save_snapshot = cli.fresh_snapshot(&dialog)?;
    let filename_field = snapshot::unique_control(
        &save_snapshot,
        "textfield",
        Some("File name:"),
        "Save As filename field",
    )?;
    let filename_ref = snapshot::control_ref(filename_field)?;
    let full_path = artifacts.file.to_str().ok_or_else(|| {
        DeskActError(
            "The unique acceptance path is not valid Unicode; no save was attempted".into(),
        )
    })?;
    cli.invoke_success(&["set-value", filename_ref, full_path], "set-value")?;

    let save_snapshot = cli.fresh_snapshot(&dialog)?;
    let filename_field = snapshot::unique_control(
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
    let save_button =
        snapshot::unique_control(&save_snapshot, "button", Some("Save"), "Save button")?;
    let save_ref = snapshot::control_ref(save_button)?;
    cli.invoke_success(&["click", save_ref], "click")?;
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
                let save_snapshot = cli.fresh_snapshot(&dialog)?;
                let filename_field = snapshot::unique_control(
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
                // The earlier save may have completed while the fresh snapshot was taken; do not click again.
                if fs::symlink_metadata(&artifacts.file).is_err() {
                    let save_button = snapshot::unique_control(
                        &save_snapshot,
                        "button",
                        Some("Save"),
                        "Save button",
                    )?;
                    cli.invoke_success(&["click", snapshot::control_ref(save_button)?], "click")?;
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
    let saved_snapshot = cli.fresh_snapshot(&saved_window)?;
    let editor = snapshot::unique_control(&saved_snapshot, "textfield", None, "saved editor")?;
    let editor_ref = snapshot::control_ref(editor)?;
    let editor_value = cli.invoke_value(&["get", editor_ref, "--property", "value"], "get")?;
    if editor_value.as_str() != Some(marker.as_str()) {
        return Err(DeskActError(
            "Fresh accessibility readback did not exactly match the fixed marker".into(),
        ));
    }

    cli.press_shortcut(&saved_window, "ctrl+w")?;
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
    // Last observation as (file exists, contents match, Save As still open), used if the deadline passes.
    let last_observed = Cell::new((false, false, false));
    poll_until_deadline(
        Instant::now() + timeout,
        |_| {
            // Observe windows first so a closed dialog is followed by a file read that reflects the completed save.
            let windows = cli.windows_for_app("notepad")?;
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
                    "Save As left an unexpected Notepad window state; no retry was attempted"
                        .into(),
                ));
            }
            last_observed.set((file_exists, contents_match, dialog_present));
            Ok((contents_match && windows.len() == 1).then_some(SaveStatus::Saved))
        },
        || {
            let (file_exists, contents_match, dialog_present) = last_observed.get();
            if dialog_present && (!file_exists || contents_match) {
                return Ok(SaveStatus::DialogStillOpen);
            }
            Err(if file_exists {
                DeskActError(
                    "Saved acceptance file contents did not exactly match the fixed marker".into(),
                )
            } else {
                DeskActError(
                    "Save As action did not produce the unique acceptance file before the verification deadline".into(),
                )
            })
        },
    )
}

fn wait_for_window_title(
    cli: &VerifiedNativeCli,
    original: &NativeWindow,
    expected_title: &str,
    pending_title: &str,
) -> Result<NativeWindow, DeskActError> {
    poll_until_deadline(
        Instant::now() + Duration::from_secs(5),
        |_| {
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
                return Ok(Some(window));
            }
            if !window.title.eq_ignore_ascii_case(pending_title) {
                return Err(DeskActError(format!(
                    "Notepad tab transition was not confirmed: expected `{expected_title}` or pending `{pending_title}`, observed `{}`",
                    window.title
                )));
            }
            Ok(None)
        },
        || {
            Err(DeskActError(format!(
                "Notepad tab did not transition to `{expected_title}` before the deadline"
            )))
        },
    )
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
    poll_until_deadline(
        Instant::now() + Duration::from_secs(5),
        |_| {
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
                    [window] => Ok(Some(window.clone())),
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
            Ok(None)
        },
        || {
            Err(DeskActError(
                "Save As dialog did not appear before the readiness deadline".into(),
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Preserve the Notepad acceptance gate's rounded-up 95% success threshold.
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
