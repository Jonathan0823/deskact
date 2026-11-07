//! Developer-only command-line harness for native preflight and inventory.

use std::{
    env,
    future::Future,
    io::{self, Write},
    process::ExitCode,
};

use deskact_core::{
    AppCatalogEntry, DeskActError, NativeTargetResolution, TaskExecutionReport, VerifiedNativeCli,
    add_user_app_entry, edit_user_app_entry, execute_action_plan, load_app_catalog,
    model_candidate_context, native_inventory, preflight, prepare_app_selection,
    registered_open_workflow, request_action_plan, request_action_plan_with_context,
    resolve_native_target, run_calculator_acceptance, run_file_explorer_acceptance,
    run_notepad_acceptance, run_vlc_acceptance, simulate_task_scenario, test_llm_connection,
    validate_action_plan_json,
};

/// Run the developer-only native preflight, inventory, target-resolution, or acceptance command.
/// Dispatch and run one developer-harness command.
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
        Some("app-catalog") => {
            let arguments: Vec<_> = env::args().skip(2).collect();
            run_app_catalog_command(&arguments)
        }
        Some("llm") => {
            let arguments: Vec<_> = env::args().skip(2).collect();
            run_llm_command(&arguments)
        }
        Some("task") => {
            let arguments: Vec<_> = env::args().skip(2).collect();
            run_task_command(&arguments)
        }
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
        Some("vlc-acceptance") => match run_vlc_acceptance() {
            Ok(()) => {
                println!("VLC acceptance verified advancing playback of the generated fixture.");
                ExitCode::SUCCESS
            }
            Err(error) => fail(error.to_string()),
        },
        Some("calculator-acceptance") => match run_calculator_acceptance() {
            Ok(()) => {
                println!(
                    "Calculator acceptance verified 7 × 6 = 42 from a fresh accessibility snapshot."
                );
                ExitCode::SUCCESS
            }
            Err(error) => fail(error.to_string()),
        },
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
                "Usage: deskact-dev <preflight|inventory|resolve <app-name>|app-catalog <list|add|edit|prepare>|llm <test|plan <request>|validate-plan>|task <simulate --scenario <name>|execute --request <request>>|notepad-acceptance [--runs <count>]|file-explorer-acceptance|calculator-acceptance|vlc-acceptance>"
            );
            ExitCode::from(2)
        }
    }
}

/// Run the local app-catalog inspection and user-entry commands.
fn run_app_catalog_command(arguments: &[String]) -> ExitCode {
    let result = match arguments {
        [command] if command == "list" => load_app_catalog().and_then(|catalog| {
            serde_json::to_string_pretty(&catalog).map_err(|error| {
                deskact_core::DeskActError::new(format!("Could not serialize app catalog: {error}"))
            })
        }),
        [command, name, target, rest @ ..] if command == "add" => match parse_aliases(rest) {
            Ok(aliases) => add_user_app_entry(name, target, aliases).and_then(|entry| {
                serde_json::to_string_pretty(&entry).map_err(|error| {
                    deskact_core::DeskActError::new(format!(
                        "Could not serialize app entry: {error}"
                    ))
                })
            }),
            Err(error) => Err(error),
        },
        [command, id, rest @ ..] if command == "edit" => match parse_edit_options(rest) {
            Ok(options) => edit_user_app_entry(
                id,
                options.name.as_deref(),
                options.target.as_deref(),
                options.aliases,
            )
            .and_then(|entry| {
                serde_json::to_string_pretty(&entry).map_err(|error| {
                    deskact_core::DeskActError::new(format!(
                        "Could not serialize app entry: {error}"
                    ))
                })
            }),
            Err(error) => Err(error),
        },
        [command, request @ ..] if command == "prepare" && !request.is_empty() => {
            let request = request.join(" ");
            load_app_catalog().and_then(|catalog| {
                let selection = prepare_app_selection(&catalog, &request)?;
                serde_json::to_string_pretty(&selection).map_err(|error| {
                    deskact_core::DeskActError::new(format!(
                        "Could not serialize app-selection input: {error}"
                    ))
                })
            })
        }
        [command, arguments @ ..] if command == "validate-selection" => {
            parse_validation_arguments(arguments).and_then(|(request, response_json)| {
                load_app_catalog().and_then(|catalog| {
                    let prepared = prepare_app_selection(&catalog, &request)?;
                    let outcome = deskact_core::validate_model_app_selection(
                        &prepared,
                        &catalog,
                        &response_json,
                    )?;
                    Ok(match outcome {
                        deskact_core::AppSelectionOutcome::Selected(entry) => {
                            format!("Validated selection: {} ({})", entry.name, entry.id)
                        }
                        deskact_core::AppSelectionOutcome::Clarify(ids) => {
                            format!("Validated clarification among: {}", ids.join(", "))
                        }
                    })
                })
            })
        }
        _ => {
            eprintln!(
                "Usage: deskact-dev app-catalog <list|add <name> <absolute-exe> [--alias <phrase> ...]|edit <id> [--name <name>] [--target <absolute-exe>] [--alias <phrase> ...|--clear-aliases]|prepare <natural-language-request>|validate-selection <natural-language-request> --response-json <fixture-json>"
            );
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("App catalog operation failed: {error}");
            ExitCode::from(1)
        }
    }
}

/// Run recovery simulation or execute one policy-bounded task plan.
fn run_task_command(arguments: &[String]) -> ExitCode {
    if let [command, flag, scenario] = arguments
        && command == "simulate"
        && flag == "--scenario"
    {
        return match simulate_task_scenario(scenario).and_then(|report| {
            serde_json::to_string_pretty(&report)
                .map_err(|_| DeskActError::new("Could not serialize task simulation report".into()))
        }) {
            Ok(report) => {
                println!("{report}");
                ExitCode::SUCCESS
            }
            Err(error) => fail(format!("Task simulation failed: {error}")),
        };
    }

    if arguments.len() >= 3 && arguments[0] == "execute" && arguments[1] == "--request" {
        let request = arguments[2..].join(" ");
        return match run_task_async(execute_task_request(&request)) {
            Ok(report) => match serde_json::to_string_pretty(&report) {
                Ok(report) => {
                    println!("{report}");
                    ExitCode::SUCCESS
                }
                Err(_) => fail("Could not serialize task execution report".into()),
            },
            Err(error) => fail(format!("Task execution failed: {error}")),
        };
    }

    eprintln!("Usage: deskact-dev task <simulate --scenario <name>|execute --request <request>");
    ExitCode::from(2)
}

async fn execute_task_request(request: &str) -> Result<TaskExecutionReport, DeskActError> {
    if request.trim().is_empty()
        || request.chars().count() > 4_000
        || request.chars().any(char::is_control)
    {
        return Err(DeskActError::new("Task request must be non-empty, at most 4,000 characters, and contain no control characters".into()));
    }
    let catalog = load_app_catalog()?;
    let (plan, route) = match registered_open_workflow(request, &catalog)? {
        Some((plan, _entry)) => (plan, "registered_workflow"),
        None => {
            let context = model_candidate_context(&catalog)?;
            let plan = request_action_plan_with_context(request, Some(&context)).await?;
            (plan, "primary_model")
        }
    };
    if !request
        .trim()
        .split_once(' ')
        .is_some_and(|(verb, app)| verb.eq_ignore_ascii_case("open") && !app.trim().is_empty())
    {
        return Err(DeskActError::new(
            "This executor currently runs only explicit `open <app>` requests; the returned plan was not executed".into(),
        ));
    }
    let mut cli = None;
    execute_action_plan(&plan, &catalog, route, |entry| {
        if cli.is_none() {
            cli = Some(VerifiedNativeCli::connect()?);
        }
        resolve_task_target(cli.as_ref().expect("CLI initialized"), entry)
    })
}

fn resolve_task_target(
    cli: &VerifiedNativeCli,
    entry: &AppCatalogEntry,
) -> Result<NativeTargetResolution, DeskActError> {
    let mut resolution = cli.resolve_target(entry.as_discovered_app())?;
    loop {
        match resolution {
            NativeTargetResolution::Ready(target) => {
                return Ok(NativeTargetResolution::Ready(target));
            }
            NativeTargetResolution::NeedsManualRestore { app, window } => {
                print!(
                    "Window `{}` for {} is minimized. Restore it manually, then press Enter to re-enumerate (or type q to cancel): ",
                    window.title, app.name
                );
                if io::stdout().flush().is_err() {
                    return Err(DeskActError::new(
                        "Could not write the restore prompt".into(),
                    ));
                }
                let mut response = String::new();
                let bytes_read = io::stdin().read_line(&mut response).map_err(|_| {
                    DeskActError::new("Could not read the restore confirmation".into())
                })?;
                if bytes_read == 0 || response.trim().eq_ignore_ascii_case("q") {
                    return Err(DeskActError::new(
                        "Resolution cancelled; no window was selected".into(),
                    ));
                }
                resolution = cli.resume_after_manual_restore(app)?;
            }
        }
    }
}

fn run_task_async<T>(
    operation: impl Future<Output = Result<T, DeskActError>>,
) -> Result<T, DeskActError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            DeskActError::new("Could not initialize the asynchronous task runtime".into())
        })?;
    runtime.block_on(operation)
}

/// Run one configured LLM connection, planning, or local-validation command.
fn run_llm_command(arguments: &[String]) -> ExitCode {
    let result = match arguments {
        [command] if command == "test" => run_llm_async(test_llm_connection()).map(|report| {
                let authentication = if report.authentication_accepted {
                    "authentication accepted"
                } else {
                    "authentication not configured"
                };
            format!(
                "LLM connection test: endpoint reachable; {authentication}; response content received; this does not guarantee action-plan compatibility."
            )
        }),
        [command, request @ ..] if command == "plan" && !request.is_empty() => {
            let request = request.join(" ");
            run_llm_async(request_action_plan(&request)).and_then(|plan| {
                serde_json::to_string_pretty(&plan).map_err(|error| {
                    DeskActError::new(format!("Could not serialize validated action plan: {error}"))
                })
            })
        }
        [command, flag, response_json]
            if command == "validate-plan" && flag == "--response-json" =>
        {
            validate_action_plan_json(response_json).map(|plan| {
                format!(
                    "Validated action plan version {} with {} step(s); this proposal is not authorization to execute.",
                    plan.version,
                    plan.steps.len()
                )
            })
        }
        _ => {
            eprintln!(
                "Usage: deskact-dev llm <test|plan <request>|validate-plan --response-json <json>>"
            );
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(error) => fail(error.to_string()),
    }
}

fn run_llm_async<T>(
    operation: impl Future<Output = Result<T, DeskActError>>,
) -> Result<T, DeskActError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| {
            DeskActError::new("Could not initialize the asynchronous HTTP runtime".into())
        })?;
    runtime.block_on(operation)
}

/// Parse repeated `--alias <phrase>` pairs for a new app entry.
fn parse_aliases(arguments: &[String]) -> Result<Vec<String>, deskact_core::DeskActError> {
    let mut aliases = Vec::new();
    let mut remaining = arguments;
    while let [flag, value, tail @ ..] = remaining {
        if flag != "--alias" {
            return Err(deskact_core::DeskActError::new(format!(
                "Unknown app-catalog add option `{flag}`"
            )));
        }
        aliases.push(value.clone());
        remaining = tail;
    }
    if !remaining.is_empty() {
        return Err(deskact_core::DeskActError::new(
            "Each --alias option requires one phrase".into(),
        ));
    }
    Ok(aliases)
}

/// Parse the original request and one fixture response separated by `--response-json`.
fn parse_validation_arguments(
    arguments: &[String],
) -> Result<(String, String), deskact_core::DeskActError> {
    let Some(separator) = arguments
        .iter()
        .position(|argument| argument == "--response-json")
    else {
        return Err(deskact_core::DeskActError::new(
            "validate-selection requires --response-json <fixture-json>".into(),
        ));
    };
    if separator == 0 || arguments.len() != separator + 2 {
        return Err(deskact_core::DeskActError::new(
            "Provide one app request before --response-json and exactly one JSON response after it"
                .into(),
        ));
    }
    Ok((
        arguments[..separator].join(" "),
        arguments[separator + 1].clone(),
    ))
}

/// Optional fields for editing one user-configured app entry.
struct AppCatalogEditOptions {
    name: Option<String>,
    target: Option<String>,
    aliases: Option<Vec<String>>,
}

/// Parse optional edit fields; providing any aliases replaces the whole optional alias set.
fn parse_edit_options(
    arguments: &[String],
) -> Result<AppCatalogEditOptions, deskact_core::DeskActError> {
    let mut name = None;
    let mut target = None;
    let mut aliases = None;
    let mut clear_aliases = false;
    let mut index = 0;
    while index < arguments.len() {
        let flag = arguments[index].as_str();
        match flag {
            "--name" | "--target" | "--alias" => {
                let Some(value) = arguments.get(index + 1) else {
                    return Err(deskact_core::DeskActError::new(format!(
                        "{flag} requires a value"
                    )));
                };
                match flag {
                    "--name" if name.is_none() => name = Some(value.clone()),
                    "--target" if target.is_none() => target = Some(value.clone()),
                    "--alias" if !clear_aliases => {
                        aliases.get_or_insert_with(Vec::new).push(value.clone());
                    }
                    "--alias" => {
                        return Err(deskact_core::DeskActError::new(
                            "--alias cannot be combined with --clear-aliases".into(),
                        ));
                    }
                    _ => {
                        return Err(deskact_core::DeskActError::new(format!(
                            "Option `{flag}` may be supplied only once"
                        )));
                    }
                }
                index += 2;
            }
            "--clear-aliases" if !clear_aliases && aliases.is_none() => {
                clear_aliases = true;
                index += 1;
            }
            _ => {
                return Err(deskact_core::DeskActError::new(format!(
                    "Unknown or conflicting app-catalog edit option `{flag}`"
                )));
            }
        }
    }
    if clear_aliases {
        aliases = Some(Vec::new());
    }
    if name.is_none() && target.is_none() && aliases.is_none() {
        return Err(deskact_core::DeskActError::new(
            "Edit requires at least one of --name, --target, --alias, or --clear-aliases".into(),
        ));
    }
    Ok(AppCatalogEditOptions {
        name,
        target,
        aliases,
    })
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
