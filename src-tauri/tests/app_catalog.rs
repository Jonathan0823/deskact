#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
    time::{SystemTime, UNIX_EPOCH},
};

use deskact_core::{
    AppCatalogEntry, AppCatalogSource, AppSelectionInput, AppSelectionOutcome,
    prepare_app_selection, validate_model_app_selection,
};

const DISCOVERED: &str = r#"{"entries":[{"name":"Calculator","target":"C:\\Windows\\System32\\calc.exe","arguments":"","working_directory":"","source":"app-paths"}],"skipped_start_menu_shortcuts":0}"#;

struct Fixture {
    dir: PathBuf,
    catalog_file: PathBuf,
    cli: PathBuf,
    powershell: PathBuf,
    calls: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "deskact-app-catalog-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create fixture directory");
        let catalog_file = dir.join("apps.json");
        let cli = dir.join("agent-desktop");
        let calls = dir.join("calls.log");
        let powershell = dir.join("powershell");
        fs::write(
            &powershell,
            r#"#!/bin/sh
case "$*" in
  *Win32_Process*) printf '%s\n' "$DESKACT_TEST_PROCESS_PATH" ;;
  *) printf '%s\n' "$DESKACT_TEST_DISCOVERY" ;;
esac
"#,
        )
        .expect("write fixture PowerShell");
        let mut permissions = fs::metadata(&powershell)
            .expect("read PowerShell metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&powershell, permissions).expect("make PowerShell executable");
        fs::write(
            &cli,
            r##"#!/bin/sh
printf '%s\n' "$*" >> "$DESKACT_TEST_CALLS"
case "$1" in
  version) echo '{"version":"2.6","ok":true,"command":"version","data":{"os":"windows","target":"x86_64","version":"1.1.0"}}' ;;
  launch) [ "$#" -eq 2 ] && [ "$2" = "$DESKACT_TEST_PROCESS_PATH" ] || { echo 'launch target was not passed as one direct argument' >&2; exit 2; }; echo '{"version":"2.6","ok":true,"command":"launch","data":{"app":"custom-player.exe","pid":9001}}' ;;
  list-windows) echo '{"version":"2.6","ok":true,"command":"list-windows","data":[{"id":"custom-window","title":"Custom Player","app_name":"custom-player.exe","accessible":true,"minimized":false,"visible":true,"pid":9001}]}' ;;
  snapshot) echo '{"version":"2.6","ok":true,"command":"snapshot","data":{"app":"custom-player.exe","window":{"id":"custom-window","title":"Custom Player"},"snapshot_id":"custom-snap","ref_count":1,"complete":true,"tree":{"role":"window","name":"Custom Player","children":[{"role":"button","name":"Open","ref_id":"@custom-snap:e1"}]}}}' ;;
  *) echo "unexpected agent-desktop command: $1" >&2; exit 2 ;;
esac
"##,
        )
        .expect("write fixture agent-desktop");
        let mut permissions = fs::metadata(&cli)
            .expect("read agent-desktop metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&cli, permissions).expect("make agent-desktop executable");
        Self {
            dir,
            catalog_file,
            cli,
            powershell,
            calls,
        }
    }

    fn target(&self, name: &str) -> PathBuf {
        let app_dir = self.dir.join("Installed Apps");
        fs::create_dir_all(&app_dir).expect("create app fixture directory");
        let target = app_dir.join(name);
        fs::write(&target, "test executable placeholder").expect("write target fixture");
        target
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with_process_path(args, "")
    }

    fn run_with_process_path(&self, args: &[&str], process_path: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .args(args)
            .env("DESKACT_APP_CATALOG", &self.catalog_file)
            .env("DESKACT_AGENT_DESKTOP", &self.cli)
            .env("DESKACT_POWERSHELL", &self.powershell)
            .env("DESKACT_TEST_CALLS", &self.calls)
            .env("DESKACT_TEST_PROCESS_PATH", process_path)
            .env("DESKACT_TEST_DISCOVERY", DISCOVERED)
            .output()
            .expect("run developer harness")
    }

    fn listed_entries(&self) -> Vec<AppCatalogEntry> {
        let output = self.run(&["app-catalog", "list"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("catalog list is JSON")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn custom_entry(id: &str, name: &str, target: &str, aliases: &[&str]) -> AppCatalogEntry {
    AppCatalogEntry {
        id: id.to_owned(),
        name: name.to_owned(),
        target: target.to_owned(),
        arguments: String::new(),
        working_directory: String::new(),
        source: AppCatalogSource::UserConfigured,
        aliases: aliases.iter().map(|alias| (*alias).to_owned()).collect(),
    }
}

/// Explicit app entries persist and remain editable without changing their stable identity.
#[test]
fn custom_entries_persist_and_can_be_edited_through_the_harness() {
    let fixture = Fixture::new();
    let spotify = fixture.target("spotify.exe");
    let vlc = fixture.target("vlc.exe");
    let target = spotify.to_str().expect("fixture path is UTF-8");
    let add = fixture.run(&["app-catalog", "add", "Spotify", target, "--alias", "music"]);
    assert!(
        add.status.success(),
        "{}",
        String::from_utf8_lossy(&add.stderr)
    );
    let added: AppCatalogEntry = serde_json::from_slice(&add.stdout).expect("added entry is JSON");
    assert_eq!(added.name, "Spotify");
    assert_eq!(added.aliases, ["music"]);
    assert_eq!(added.source, AppCatalogSource::UserConfigured);

    let before = fixture.listed_entries();
    assert!(before.iter().any(|entry| entry.id == added.id));
    assert!(before.iter().any(|entry| entry.name == "Calculator"));

    let edited = fixture.run(&[
        "app-catalog",
        "edit",
        &added.id,
        "--name",
        "Spotify Desktop",
        "--target",
        vlc.to_str().expect("fixture path is UTF-8"),
        "--alias",
        "music",
        "--alias",
        "songs",
    ]);
    assert!(
        edited.status.success(),
        "{}",
        String::from_utf8_lossy(&edited.stderr)
    );
    let updated: AppCatalogEntry =
        serde_json::from_slice(&edited.stdout).expect("edited entry is JSON");
    assert_eq!(updated.id, added.id);
    assert_eq!(updated.name, "Spotify Desktop");
    assert_eq!(updated.target, vlc.to_str().unwrap());
    assert_eq!(updated.aliases, ["music", "songs"]);

    let after = fixture.listed_entries();
    assert!(
        after
            .iter()
            .any(|entry| entry.id == added.id && entry.name == "Spotify Desktop")
    );
}

/// Alias collisions are rejected instead of silently resolving to whichever entry came first.
#[test]
fn conflicting_aliases_are_rejected_and_existing_catalog_is_unchanged() {
    let fixture = Fixture::new();
    let first = fixture.target("first.exe");
    let second = fixture.target("second.exe");
    let first_output = fixture.run(&[
        "app-catalog",
        "add",
        "First Player",
        first.to_str().unwrap(),
        "--alias",
        "music",
    ]);
    assert!(first_output.status.success());

    let collision = fixture.run(&[
        "app-catalog",
        "add",
        "Second Player",
        second.to_str().unwrap(),
        "--alias",
        "MUSIC",
    ]);
    assert!(!collision.status.success());
    assert!(
        String::from_utf8_lossy(&collision.stderr).contains("conflicts"),
        "{}",
        String::from_utf8_lossy(&collision.stderr)
    );
    assert_eq!(
        fixture
            .listed_entries()
            .iter()
            .filter(|entry| entry.source == AppCatalogSource::UserConfigured)
            .count(),
        1
    );
}

/// User aliases cannot shadow a canonical app discovered from Windows.
#[test]
fn aliases_conflicting_with_discovered_app_names_are_rejected() {
    let fixture = Fixture::new();
    let target = fixture.target("custom.exe");
    let output = fixture.run(&[
        "app-catalog",
        "add",
        "Custom App",
        target.to_str().unwrap(),
        "--alias",
        "calculator",
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("conflicts"));
    assert_eq!(fixture.listed_entries().len(), 1);
}

/// Custom targets must be existing absolute executable files, not shell fragments or guessed paths.
#[test]
fn custom_target_validation_rejects_nonexistent_and_non_executable_targets() {
    let fixture = Fixture::new();
    let missing = fixture.dir.join("missing.exe");
    let output = fixture.run(&["app-catalog", "add", "Missing", missing.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("existing .exe"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let text_file = fixture.dir.join("not-an-app.txt");
    fs::write(&text_file, "not executable").unwrap();
    let output = fixture.run(&[
        "app-catalog",
        "add",
        "Text file",
        text_file.to_str().unwrap(),
    ]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("existing .exe"));
}

/// Natural-language selection sees IDs and display names only, never executable paths.
#[test]
fn non_exact_request_builds_a_catalog_only_model_candidate_set() {
    let catalog = vec![
        custom_entry(
            "custom-1",
            "Visual Studio Code",
            r"C:\Apps\Code.exe",
            &["vscode"],
        ),
        custom_entry("custom-2", "Notepad++", r"C:\Apps\Notepad++.exe", &[]),
    ];
    let input = prepare_app_selection(&catalog, "open my editor").expect("prepare model input");
    let AppSelectionInput::NeedsModel {
        request,
        candidates,
    } = input
    else {
        panic!("non-exact request must use the primary model");
    };
    assert_eq!(request, "open my editor");
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].id, "custom-1");
    assert_eq!(candidates[0].display_name, "Visual Studio Code");
    let serialized = serde_json::to_string(&candidates).expect("serialize candidates");
    assert!(!serialized.contains(r"C:\Apps\Code.exe"), "{serialized}");
    assert!(!serialized.contains("target"), "{serialized}");
}

/// Exact canonical names and explicitly saved aliases remain a deterministic fast path.
#[test]
fn exact_names_and_optional_aliases_resolve_without_model_selection() {
    let catalog = vec![
        custom_entry("custom-1", "Spotify", r"C:\Apps\Spotify.exe", &["music"]),
        custom_entry("custom-2", "VLC", r"C:\Apps\vlc.exe", &[]),
    ];
    let exact = prepare_app_selection(&catalog, "Spotify").expect("exact selection");
    assert!(matches!(exact, AppSelectionInput::ExactMatch { ref entry } if entry.id == "custom-1"));
    let alias = prepare_app_selection(&catalog, "music").expect("alias selection");
    assert!(matches!(alias, AppSelectionInput::ExactMatch { ref entry } if entry.id == "custom-1"));

    let ambiguous = vec![
        custom_entry("custom-3", "Editor", r"C:\\Apps\\Editor.exe", &[]),
        custom_entry("custom-4", "Editor", r"D:\\Apps\\Editor.exe", &[]),
    ];
    let clarification =
        prepare_app_selection(&ambiguous, "editor").expect("candidate clarification");
    assert!(
        matches!(clarification, AppSelectionInput::NeedsClarification { candidates } if candidates.len() == 2)
    );
}

/// The CLI exposes model-safe candidate JSON for natural-language requests and exact local matches.
#[test]
fn prepare_command_outputs_catalog_candidates_without_launch_targets() {
    let fixture = Fixture::new();
    let target = fixture.target("editor.exe");
    let added = fixture.run(&[
        "app-catalog",
        "add",
        "Visual Studio Code",
        target.to_str().unwrap(),
        "--alias",
        "code",
    ]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let entry: AppCatalogEntry = serde_json::from_slice(&added.stdout).unwrap();

    let output = fixture.run(&["app-catalog", "prepare", "open my editor"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let prepared: AppSelectionInput = serde_json::from_slice(&output.stdout).unwrap();
    let AppSelectionInput::NeedsModel { candidates, .. } = prepared else {
        panic!("natural-language request should use the model-selection seam");
    };
    assert!(candidates.iter().any(|candidate| candidate.id == entry.id));
    let candidate_json = serde_json::to_string(&candidates).unwrap();
    assert!(!candidate_json.contains(target.to_str().unwrap()));

    let exact = fixture.run(&["app-catalog", "prepare", "code"]);
    let exact: AppSelectionInput = serde_json::from_slice(&exact.stdout).unwrap();
    assert!(
        matches!(exact, AppSelectionInput::ExactMatch { entry: ref matched } if matched.id == entry.id)
    );
}

/// User-configured app resolution shares the fresh-window, owner, and accessibility gates.
#[test]
fn exact_custom_app_resolution_uses_safe_direct_launch_and_fresh_window_verification() {
    let fixture = Fixture::new();
    let target = fixture.target("custom-player.exe");
    let target = target.to_str().unwrap();
    let added = fixture.run(&["app-catalog", "add", "Custom Player", target]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );

    let output = fixture.run_with_process_path(&["resolve", "Custom Player"], target);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("Custom Player"));
    let calls = fs::read_to_string(&fixture.calls).unwrap();
    assert!(calls.contains(&format!("launch {target}")), "{calls}");
    assert!(calls.contains("list-windows --timeout-ms"), "{calls}");
    assert!(
        calls.contains("snapshot --app custom-player.exe --window-id custom-window -i"),
        "{calls}"
    );
}

/// A model response can select only a supplied ID or request clarification among supplied IDs.
#[test]
fn model_selection_rejects_unknown_ids_and_accepts_valid_selection_or_clarification() {
    let catalog = vec![
        custom_entry("custom-1", "Visual Studio Code", r"C:\Apps\Code.exe", &[]),
        custom_entry("custom-2", "Notepad++", r"C:\Apps\Notepad++.exe", &[]),
    ];
    let input = prepare_app_selection(&catalog, "open my editor").expect("prepare model input");

    let selected = validate_model_app_selection(
        &input,
        &catalog,
        r#"{"decision":"select","candidate_id":"custom-1"}"#,
    )
    .expect("validate model selection");
    assert!(matches!(selected, AppSelectionOutcome::Selected(ref entry) if entry.id == "custom-1"));

    let clarification = validate_model_app_selection(
        &input,
        &catalog,
        r#"{"decision":"clarify","candidate_ids":["custom-1","custom-2"]}"#,
    )
    .expect("validate model clarification");
    assert!(
        matches!(clarification, AppSelectionOutcome::Clarify(ref ids) if ids == &["custom-1", "custom-2"])
    );

    for response in [
        r#"{"decision":"select","candidate_id":"invented"}"#,
        r#"{"decision":"select","candidate_id":"custom-1","target":"C:\\evil.exe"}"#,
        r#"{"decision":"clarify","candidate_ids":["custom-1","invented"]}"#,
        r#"{"decision":"clarify","candidate_ids":["custom-1","custom-1"]}"#,
        r#"{"decision":"clarify","candidate_ids":["custom-1"]}"#,
    ] {
        assert!(
            validate_model_app_selection(&input, &catalog, response).is_err(),
            "accepted unsafe model response: {response}"
        );
    }
}
