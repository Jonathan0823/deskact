//! Developer-only command-line harness for native preflight and inventory.

use std::{
    env,
    io::{self, Write},
    process::ExitCode,
};

use deskact_core::{
    NativeTargetResolution, VerifiedNativeCli, native_inventory, preflight, resolve_native_target,
    run_file_explorer_acceptance, run_notepad_acceptance,
};

/// Run the developer-only native preflight, inventory, target-resolution, or acceptance command.
fn main() -> ExitCode {
    let command = env::args().nth(1);
    match command.as_deref() {
        Some("preflight") => match preflight() {
            Ok(status) => {
                println!(
                    "agent-desktop {} (JSON contract {}) is ready",
                    status.tool_version, status.contract_version
                );
                ExitCode::SUCCESS
            }
            Err(error) => fail(error.to_string()),
        },
        Some("resolve") => {
            let mut arguments = env::args().skip(2);
            match (arguments.next(), arguments.next()) {
                (Some(app_name), None) => match resolve_native_target(&app_name) {
                    Ok(resolution) => finish_resolution(resolution),
                    Err(error) => fail(error.to_string()),
                },
                _ => {
                    eprintln!("Usage: deskact-dev resolve <discovered-app-name>");
                    ExitCode::from(2)
                }
            }
        }
        Some("file-explorer-acceptance") => match run_file_explorer_acceptance() {
            Ok(()) => {
                println!("File Explorer acceptance verified the fixture path and filename.");
                ExitCode::SUCCESS
            }
            Err(error) => fail(error.to_string()),
        },
        Some("notepad-acceptance") => {
            let arguments: Vec<_> = env::args().skip(2).collect();
            let runs = match arguments.as_slice() {
                [] => Some(20),
                [flag, count] if flag == "--runs" => {
                    count.parse::<usize>().ok().filter(|runs| *runs > 0)
                }
                _ => None,
            };
            match runs {
                Some(runs) => match run_notepad_acceptance(runs) {
                    Ok(report) => {
                        if report.gate_met {
                            println!(
                                "Controlled Notepad acceptance verified {}/{} attempted runs ({} of {} required; acceptance gate met).",
                                report.verified,
                                report.attempted,
                                report.required,
                                report.requested
                            );
                        } else {
                            println!(
                                "Controlled Notepad smoke run verified {}/{} runs; the 19/20 acceptance gate was not assessed.",
                                report.verified, report.attempted
                            );
                        }
                        for failure in report.failures {
                            eprintln!("Unverified: {failure}");
                        }
                        ExitCode::SUCCESS
                    }
                    Err(error) => fail(error.to_string()),
                },
                None => {
                    eprintln!("Usage: deskact-dev notepad-acceptance [--runs <count>]");
                    ExitCode::from(2)
                }
            }
        }
        Some("inventory") => match native_inventory() {
            Ok(inventory) => {
                println!(
                    "agent-desktop {} (JSON contract {}) is ready",
                    inventory.preflight.tool_version, inventory.preflight.contract_version
                );
                println!("Discovered app entries: {}", inventory.catalog.len());
                println!(
                    "Skipped Start Menu shortcuts without file targets: {}",
                    inventory.skipped_start_menu_shortcuts
                );
                for app in &inventory.catalog {
                    println!("- {} [{}] -> {}", app.name, app.source, app.target);
                }
                println!(
                    "Currently running GUI apps: {}",
                    inventory.running_apps.len()
                );
                if inventory.running_apps.is_empty() {
                    println!("No running GUI applications found.");
                } else {
                    for app in inventory.running_apps {
                        println!("- {} (PID {}, {})", app.name, app.pid, app.presentation);
                    }
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(error.to_string()),
        },

        _ => {
            eprintln!(
                "Usage: deskact-dev <preflight|inventory|resolve <discovered-app-name>|notepad-acceptance [--runs <count>]|file-explorer-acceptance>"
            );
            ExitCode::from(2)
        }
    }
}

/// Present a resolved target or pause for manual restoration and then resume resolution.
fn finish_resolution(mut resolution: NativeTargetResolution) -> ExitCode {
    loop {
        match resolution {
            NativeTargetResolution::Ready(target) => {
                println!(
                    "Resolved {} to accessible window `{}` ({}); fresh snapshot {} has {} actionable controls.",
                    target.app.name,
                    target.window.title,
                    target.window.id,
                    target.snapshot_id,
                    target.ref_count
                );
                return ExitCode::SUCCESS;
            }
            NativeTargetResolution::NeedsManualRestore { app, window } => {
                print!(
                    "Window `{}` for {} is minimized. Restore it manually, then press Enter to re-enumerate (or type q to cancel): ",
                    window.title, app.name
                );
                if io::stdout().flush().is_err() {
                    return fail("Could not write the restore prompt".into());
                }
                let mut response = String::new();
                let bytes_read = match io::stdin().read_line(&mut response) {
                    Ok(bytes_read) => bytes_read,
                    Err(_) => return fail("Could not read the restore confirmation".into()),
                };
                if bytes_read == 0 || response.trim().eq_ignore_ascii_case("q") {
                    eprintln!("Resolution cancelled; no window was selected.");
                    return ExitCode::from(1);
                }
                resolution = match VerifiedNativeCli::connect()
                    .and_then(|cli| cli.resume_after_manual_restore(app))
                {
                    Ok(resolution) => resolution,
                    Err(error) => return fail(error.to_string()),
                };
            }
        }
    }
}

/// Print an actionable harness error and return a non-success exit code.
fn fail(message: String) -> ExitCode {
    eprintln!("Native desktop capability unavailable: {message}");
    ExitCode::from(1)
}
