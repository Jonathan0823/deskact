use std::{env, fs::OpenOptions, io::Write, path::Path};

/// Emit fixture responses for the native CLI and the PowerShell process/catalog probes.
fn main() {
    let arguments: Vec<_> = env::args().skip(1).collect();
    log_invocation(&arguments);

    if arguments.first().is_some_and(|argument| argument == "-NoProfile") {
        run_powershell_fixture(&arguments);
        return;
    }

    match arguments.first().map(String::as_str) {
        Some("version") => println!(r#"{{"version":"2.6","ok":true,"command":"version","data":{{"os":"windows","target":"x86_64","version":"1.1.0"}}}}"#),
        Some("launch") => println!(r#"{{"version":"2.6","ok":true,"command":"launch","data":{{"launched":true}}}}"#),
        Some("list-windows") => {
            let accessible = env::var("DESKACT_TEST_ACCESSIBLE").unwrap_or_else(|_| "true".into());
            let minimized = env::var("DESKACT_TEST_MINIMIZED").unwrap_or_else(|_| "false".into());
            println!(r#"{{"version":"2.6","ok":true,"command":"list-windows","data":[{{"id":"fixture-window","title":"Calculator","app_name":"Calculator.exe","accessible":{accessible},"minimized":{minimized},"is_focused":false,"visible":true,"pid":4512}}]}}"#);
        }
        Some("snapshot") => println!(r#"{{"version":"2.6","ok":true,"command":"snapshot","data":{{"window":{{"id":"fixture-window"}},"complete":true,"ref_count":1,"snapshot_id":"fixture-snapshot","tree":{{"role":"window","children":[]}}}}}}"#),
        _ => {
            eprintln!("unexpected fake agent-desktop arguments: {arguments:?}");
            std::process::exit(2);
        }
    }
}

/// Emulate the app-catalog discovery and process-image PowerShell queries.
fn run_powershell_fixture(arguments: &[String]) {
    let Some(script) = arguments.last() else {
        std::process::exit(2);
    };
    if script.contains("Get-CimInstance") {
        println!("{}", env::var("DESKACT_TEST_TARGET").expect("fixture target"));
    } else {
        print!(r#"{{"entries":[],"skipped_start_menu_shortcuts":0}}"#);
    }
}

/// Append the received argument vector to the test-owned call log.
fn log_invocation(arguments: &[String]) {
    let Some(path) = env::var_os("DESKACT_TEST_CALLS") else {
        return;
    };
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(Path::new(&path))
        .expect("open fixture call log");
    writeln!(file, "{}", arguments.join(" ")).expect("write fixture call log");
}
