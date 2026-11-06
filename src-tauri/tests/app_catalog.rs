#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output, Stdio},
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
  *) sleep "${DESKACT_TEST_DISCOVERY_DELAY:-0}"; printf '%s\n' "$DESKACT_TEST_DISCOVERY" ;;
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
        self.run_with_fixture(args, "", DISCOVERED)
    }

    fn run_with_process_path(&self, args: &[&str], process_path: &str) -> Output {
        self.run_with_fixture(args, process_path, DISCOVERED)
    }

    fn run_with_discovery(&self, args: &[&str], discovery: &str) -> Output {
        self.run_with_fixture(args, "", discovery)
    }

    fn run_with_fixture(&self, args: &[&str], process_path: &str, discovery: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
            .args(args)
            .env("DESKACT_APP_CATALOG", &self.catalog_file)
            .env("DESKACT_AGENT_DESKTOP", &self.cli)
            .env("DESKACT_POWERSHELL", &self.powershell)
            .env("DESKACT_TEST_CALLS", &self.calls)
            .env("DESKACT_TEST_PROCESS_PATH", process_path)
            .env("DESKACT_TEST_DISCOVERY", discovery)
            .env("DESKACT_TEST_DISCOVERY_DELAY", "")
            .output()
            .expect("run developer harness")
    }

    fn write_entries(&self, entries: &[AppCatalogEntry]) {
        let store = serde_json::json!({ "version": 1, "entries": entries });
        fs::write(
            &self.catalog_file,
            serde_json::to_vec_pretty(&store).unwrap(),
        )
        .expect("write app catalog fixture");
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

/// Case-sensitive shortcut arguments remain part of discovered identity and are not deduplicated.
#[test]
fn discovered_entries_with_argument_case_differences_keep_distinct_ids() {
    let fixture = Fixture::new();
    let discovery = r#"{"entries":[{"name":"Editor","target":"C:\\Apps\\editor.exe","arguments":"--Profile Alpha","working_directory":"","source":"start-menu"},{"name":"Editor","target":"C:\\Apps\\editor.exe","arguments":"--profile Alpha","working_directory":"","source":"start-menu"}],"skipped_start_menu_shortcuts":0}"#;
    let output = fixture.run_with_discovery(&["app-catalog", "list"], discovery);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let entries: Vec<AppCatalogEntry> = serde_json::from_slice(&output.stdout).unwrap();
    let editors: Vec<_> = entries
        .iter()
        .filter(|entry| entry.name == "Editor")
        .collect();
    assert_eq!(editors.len(), 2);
    assert_ne!(editors[0].id, editors[1].id);
    assert_ne!(editors[0].arguments, editors[1].arguments);
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

/// Concurrent processes add entries without overwriting each other's catalog updates.
#[test]
fn concurrent_app_catalog_updates_are_serialized_across_processes() {
    use std::process::Child;

    let fixture = Fixture::new();
    let mut children: Vec<Child> = (0..6)
        .map(|index| {
            let target = fixture.target(&format!("parallel-{index}.exe"));
            Command::new(env!("CARGO_BIN_EXE_deskact-dev"))
                .args([
                    "app-catalog",
                    "add",
                    &format!("Parallel App {index}"),
                    target.to_str().unwrap(),
                ])
                .env("DESKACT_APP_CATALOG", &fixture.catalog_file)
                .env("DESKACT_POWERSHELL", &fixture.powershell)
                .env("DESKACT_TEST_DISCOVERY", DISCOVERED)
                .env("DESKACT_TEST_DISCOVERY_DELAY", "0.1")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn concurrent catalog update")
        })
        .collect();
    for child in &mut children {
        assert!(child.wait().unwrap().success());
    }
    let configured = fixture
        .listed_entries()
        .into_iter()
        .filter(|entry| entry.source == AppCatalogSource::UserConfigured)
        .count();
    assert_eq!(configured, 6);
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

/// Existing conflicts remain visible and repairable, while exact conflicted names require clarification.
#[test]
fn existing_alias_conflicts_do_not_block_listing_selection_or_repair() {
    let fixture = Fixture::new();
    let first_target = fixture.target("first.exe");
    let second_target = fixture.target("second.exe");
    fixture.write_entries(&[
        custom_entry(
            "custom-first",
            "First Player",
            first_target.to_str().unwrap(),
            &["shared"],
        ),
        custom_entry(
            "custom-second",
            "Second Player",
            second_target.to_str().unwrap(),
            &["shared"],
        ),
    ]);

    let listed = fixture.listed_entries();
    assert!(listed.iter().any(|entry| entry.id == "custom-first"));
    let exact = fixture.run(&["app-catalog", "prepare", "Calculator"]);
    assert!(
        exact.status.success(),
        "{}",
        String::from_utf8_lossy(&exact.stderr)
    );
    let exact: AppSelectionInput = serde_json::from_slice(&exact.stdout).unwrap();
    assert!(matches!(exact, AppSelectionInput::ExactMatch { entry } if entry.name == "Calculator"));

    let conflicted = fixture.run(&["app-catalog", "prepare", "shared"]);
    assert!(
        conflicted.status.success(),
        "{}",
        String::from_utf8_lossy(&conflicted.stderr)
    );
    let conflicted: AppSelectionInput = serde_json::from_slice(&conflicted.stdout).unwrap();
    assert!(
        matches!(conflicted, AppSelectionInput::NeedsClarification { candidates } if candidates.len() == 2)
    );
    let resolution = fixture.run(&["resolve", "shared"]);
    assert!(!resolution.status.success());
    assert!(String::from_utf8_lossy(&resolution.stderr).contains("ask the user"));
    assert!(
        !fs::read_to_string(&fixture.calls)
            .unwrap_or_default()
            .contains("launch")
    );

    let introduced = fixture.run(&[
        "app-catalog",
        "edit",
        "custom-first",
        "--alias",
        "Second Player",
    ]);
    assert!(!introduced.status.success());
    assert!(
        fixture
            .listed_entries()
            .iter()
            .any(|entry| { entry.id == "custom-first" && entry.aliases == ["shared"] })
    );

    let repaired = fixture.run(&[
        "app-catalog",
        "edit",
        "custom-first",
        "--alias",
        "first-only",
    ]);
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );
    let after = fixture.listed_entries();
    assert!(
        after
            .iter()
            .any(|entry| entry.id == "custom-first" && entry.aliases == ["first-only"])
    );
}

/// Alias and clear options are rejected in either order without modifying persisted aliases.
#[test]
fn alias_and_clear_alias_options_conflict_in_both_orders() {
    let fixture = Fixture::new();
    let target = fixture.target("player.exe");
    let entry = custom_entry(
        "custom-player",
        "Player",
        target.to_str().unwrap(),
        &["old"],
    );
    fixture.write_entries(std::slice::from_ref(&entry));
    for options in [
        ["--clear-aliases", "--alias", "new"],
        ["--alias", "new", "--clear-aliases"],
    ] {
        let output = fixture.run(&[
            "app-catalog",
            "edit",
            "custom-player",
            options[0],
            options[1],
            options[2],
        ]);
        assert!(!output.status.success());
        let current = fixture.listed_entries();
        let current = current
            .iter()
            .find(|item| item.id == "custom-player")
            .unwrap();
        assert_eq!(current.aliases, ["old"]);
    }
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

/// The harness validates fixture model responses against the freshly prepared local candidate set.
#[test]
fn validate_selection_command_accepts_valid_choices_and_rejects_unsafe_responses() {
    let fixture = Fixture::new();
    let target = fixture.target("editor.exe");
    let added = fixture.run(&["app-catalog", "add", "My Editor", target.to_str().unwrap()]);
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    let entry: AppCatalogEntry = serde_json::from_slice(&added.stdout).unwrap();
    let prepared = fixture.run(&["app-catalog", "prepare", "open my editor"]);
    let prepared: AppSelectionInput = serde_json::from_slice(&prepared.stdout).unwrap();
    let AppSelectionInput::NeedsModel { candidates, .. } = prepared else {
        panic!("request should require model selection");
    };
    let second_id = candidates
        .iter()
        .find(|candidate| candidate.id != entry.id)
        .unwrap()
        .id
        .clone();

    let selected_json = format!(r#"{{"decision":"select","candidate_id":"{}"}}"#, entry.id);
    let selected = fixture.run(&[
        "app-catalog",
        "validate-selection",
        "open",
        "my",
        "editor",
        "--response-json",
        &selected_json,
    ]);
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    assert!(String::from_utf8_lossy(&selected.stdout).contains(&entry.id));

    let clarification_json = format!(
        r#"{{"decision":"clarify","candidate_ids":["{}","{}"]}}"#,
        entry.id, second_id
    );
    let clarification = fixture.run(&[
        "app-catalog",
        "validate-selection",
        "open",
        "my",
        "editor",
        "--response-json",
        &clarification_json,
    ]);
    assert!(
        clarification.status.success(),
        "{}",
        String::from_utf8_lossy(&clarification.stderr)
    );
    assert!(String::from_utf8_lossy(&clarification.stdout).contains("Validated clarification"));

    for response in [
        r#"{"decision":"select","candidate_id":"outside"}"#,
        r#"{"decision":"select","candidate_id":"custom-player","target":"C:\\evil.exe"}"#,
    ] {
        let rejected = fixture.run(&[
            "app-catalog",
            "validate-selection",
            "open",
            "my",
            "editor",
            "--response-json",
            response,
        ]);
        assert!(!rejected.status.success(), "accepted response: {response}");
    }
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

    let AppSelectionInput::NeedsModel { request, .. } = &input else {
        panic!("expected a model candidate input");
    };
    let subset = AppSelectionInput::NeedsModel {
        request: request.clone(),
        candidates: vec![deskact_core::AppCandidate {
            id: "custom-1".into(),
            display_name: "Visual Studio Code".into(),
        }],
    };
    for response in [
        r#"{"decision":"select","candidate_id":"custom-2"}"#,
        r#"{"decision":"clarify","candidate_ids":["custom-1","custom-2"]}"#,
    ] {
        assert!(validate_model_app_selection(&subset, &catalog, response).is_err());
    }

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
