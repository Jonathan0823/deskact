//! Developer-only command-line harness for native preflight and inventory.

use std::{
    env,
    io::{self, Write},
    process::ExitCode,
};

use deskact_core::{
    NativeTargetResolution, VerifiedNativeCli, native_inventory, preflight, resolve_native_target,
};

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
        Some("resolve") => match env::args().nth(2) {
            Some(app_name) => match resolve_native_target(&app_name) {
                Ok(resolution) => finish_resolution(resolution),
                Err(error) => fail(error.to_string()),
            },
            None => {
                eprintln!("Usage: deskact-dev resolve <discovered-app-name>");
                ExitCode::from(2)
            }
        },
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
            eprintln!("Usage: deskact-dev <preflight|inventory|resolve <discovered-app-name>>");
            ExitCode::from(2)
        }
    }
}

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

fn fail(message: String) -> ExitCode {
    eprintln!("Native desktop capability unavailable: {message}");
    ExitCode::from(1)
}
