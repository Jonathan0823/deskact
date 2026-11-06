//! Persistent user app entries and catalog-grounded app selection.

use std::{
    env, fs,
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::{DeskActError, DiscoveredApp, DiscoverySource, windows_catalog::discover_app_catalog};

/// Current on-disk app catalog schema version.
const STORE_VERSION: u32 = 1;
/// In-process sequence used to disambiguate user-entry IDs created in the same clock tick.
static USER_ENTRY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Origin of a candidate in DeskAct's app catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AppCatalogSource {
    /// Discovered from a Windows Start Menu shortcut.
    StartMenu,
    /// Discovered from a registered Windows App Paths entry.
    AppPaths,
    /// Explicitly configured by the user in DeskAct.
    UserConfigured,
}

impl std::fmt::Display for AppCatalogSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::StartMenu => "start-menu",
            Self::AppPaths => "app-paths",
            Self::UserConfigured => "user-configured",
        })
    }
}

/// One app identity and its locally authoritative launch target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppCatalogEntry {
    /// Stable ID used to ground model selections; never supplied by the model.
    pub id: String,
    /// App name displayed to the user and model.
    pub name: String,
    /// Executable or discovered app target; never sent in the model candidate list.
    pub target: String,
    /// Arguments discovered from a shortcut; configured entries use no arbitrary arguments.
    pub arguments: String,
    /// Working directory discovered from a shortcut, when available.
    pub working_directory: String,
    /// Discovery source or explicit user configuration.
    pub source: AppCatalogSource,
    /// Optional explicit exact-match phrases; never inferred or learned.
    pub aliases: Vec<String>,
}

impl AppCatalogEntry {
    /// Convert a discovered Windows application into a catalog entry with a stable ID.
    pub(crate) fn from_discovered(app: DiscoveredApp) -> Self {
        let id = discovered_entry_id(&app);
        let source = match app.source {
            DiscoverySource::StartMenu => AppCatalogSource::StartMenu,
            DiscoverySource::AppPaths => AppCatalogSource::AppPaths,
            DiscoverySource::UserConfigured => AppCatalogSource::UserConfigured,
        };
        Self {
            id,
            name: app.name,
            target: app.target,
            arguments: app.arguments,
            working_directory: app.working_directory,
            source,
            aliases: Vec::new(),
        }
    }

    /// Restore the native resolver's app shape from a locally authoritative catalog entry.
    pub(crate) fn as_discovered_app(&self) -> DiscoveredApp {
        let source = match self.source {
            AppCatalogSource::StartMenu => DiscoverySource::StartMenu,
            AppCatalogSource::AppPaths => DiscoverySource::AppPaths,
            AppCatalogSource::UserConfigured => DiscoverySource::UserConfigured,
        };
        DiscoveredApp {
            name: self.name.clone(),
            target: self.target.clone(),
            arguments: self.arguments.clone(),
            working_directory: self.working_directory.clone(),
            source,
        }
    }
}

/// Information intentionally exposed to the primary model for app resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppCandidate {
    /// Opaque local catalog ID accepted by the validator.
    pub id: String,
    /// Display name only; launch targets and aliases are kept local.
    pub display_name: String,
}

/// Input shape for either an exact deterministic match or primary-model selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppSelectionInput {
    /// Unique canonical name or explicit alias matched locally.
    ExactMatch {
        /// The locally validated entry.
        entry: AppCatalogEntry,
    },
    /// Candidate list for a non-exact natural-language app reference.
    NeedsModel {
        /// Original user request.
        request: String,
        /// Eligible candidates; intentionally excludes targets and arguments.
        candidates: Vec<AppCandidate>,
    },
    /// More than one exact canonical name or alias matched.
    NeedsClarification {
        /// Candidate IDs and names to present to the user.
        candidates: Vec<AppCandidate>,
    },
}

/// A constrained response expected from the primary model for non-exact app selection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelAppSelection {
    /// Select one catalog entry by its supplied ID.
    Select {
        /// ID copied from the current candidate set.
        candidate_id: String,
    },
    /// Ask the user to choose among supplied candidate IDs.
    Clarify {
        /// Two or more IDs copied from the current candidate set.
        candidate_ids: Vec<String>,
    },
}

/// Locally validated result of primary-model app selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppSelectionOutcome {
    /// A catalog entry selected by an exact match or validated model response.
    Selected(AppCatalogEntry),
    /// Candidate IDs to present to the user; no app is launched yet.
    Clarify(Vec<String>),
}

/// Versioned persisted representation of explicitly configured app entries.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppCatalogStore {
    version: u32,
    entries: Vec<AppCatalogEntry>,
}

/// Cross-process exclusive lock held while a catalog update is read, validated, and written.
struct CatalogLock {
    file: fs::File,
}

impl Drop for CatalogLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Load current discovered apps and explicitly configured local app entries.
pub fn load_app_catalog() -> Result<Vec<AppCatalogEntry>, DeskActError> {
    let discovered = crate::normalize_catalog(discover_app_catalog()?.entries)
        .into_iter()
        .map(AppCatalogEntry::from_discovered);
    let configured = read_user_entries()?;
    let mut entries: Vec<_> = discovered.chain(configured).collect();
    entries.sort_by_key(|entry| (entry.name.to_lowercase(), entry.id.clone()));
    ensure_unique_catalog_ids(&entries)?;
    Ok(entries)
}

/// Prepare an exact fast path or a privacy-minimal candidate set for model selection.
pub fn prepare_app_selection(
    catalog: &[AppCatalogEntry],
    request: &str,
) -> Result<AppSelectionInput, DeskActError> {
    ensure_unique_catalog_ids(catalog)?;
    let request = request.trim();
    if request.is_empty() {
        return Err(DeskActError::new("App request must not be empty".into()));
    }
    let matching: Vec<_> = catalog
        .iter()
        .filter(|entry| {
            app_name_key(&entry.name) == app_name_key(request)
                || entry
                    .aliases
                    .iter()
                    .any(|alias| app_name_key(alias) == app_name_key(request))
        })
        .collect();
    match matching.as_slice() {
        [entry] => Ok(AppSelectionInput::ExactMatch {
            entry: (*entry).clone(),
        }),
        [_, _, ..] => Ok(AppSelectionInput::NeedsClarification {
            candidates: matching.iter().map(|entry| candidate(entry)).collect(),
        }),
        [] if catalog.is_empty() => Err(DeskActError(
            "No discovered or configured app candidates are available".into(),
        )),
        [] => Ok(AppSelectionInput::NeedsModel {
            request: request.to_owned(),
            candidates: catalog.iter().map(candidate).collect(),
        }),
    }
}

/// Validate model output against the exact candidate set sent for this request.
pub fn validate_model_app_selection(
    input: &AppSelectionInput,
    catalog: &[AppCatalogEntry],
    response_json: &str,
) -> Result<AppSelectionOutcome, DeskActError> {
    let AppSelectionInput::NeedsModel { candidates, .. } = input else {
        return Err(DeskActError(
            "A model selection can be validated only for a non-exact app request".into(),
        ));
    };
    let response: ModelAppSelection = serde_json::from_str(response_json).map_err(|error| {
        DeskActError(format!(
            "Primary model returned an invalid app-selection response: {error}"
        ))
    })?;
    ensure_unique_catalog_ids(catalog)?;
    let entries_by_id: std::collections::HashMap<_, _> = catalog
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let mut allowed_candidate_ids = std::collections::HashSet::new();
    for candidate in candidates {
        let Some(entry) = entries_by_id.get(candidate.id.as_str()) else {
            return Err(DeskActError(
                "App-selection input contains a candidate outside the local app catalog".into(),
            ));
        };
        if entry.name != candidate.display_name
            || !allowed_candidate_ids.insert(candidate.id.as_str())
        {
            return Err(DeskActError(
                "App-selection input contains a duplicate or stale catalog candidate".into(),
            ));
        }
    }
    match response {
        ModelAppSelection::Select { candidate_id } => {
            let Some(entry) = entries_by_id.get(candidate_id.as_str()) else {
                return Err(DeskActError(
                    "Primary model selected an app ID outside the local app catalog".into(),
                ));
            };
            if !allowed_candidate_ids.contains(candidate_id.as_str()) {
                return Err(DeskActError(
                    "Primary model selected an app ID outside the supplied catalog candidates"
                        .into(),
                ));
            }
            Ok(AppSelectionOutcome::Selected((*entry).clone()))
        }
        ModelAppSelection::Clarify { candidate_ids } => {
            if candidate_ids.len() < 2 {
                return Err(DeskActError(
                    "Primary model clarification must include at least two catalog candidates"
                        .into(),
                ));
            }
            let mut seen = std::collections::HashSet::new();
            for id in &candidate_ids {
                if !entries_by_id.contains_key(id.as_str())
                    || !allowed_candidate_ids.contains(id.as_str())
                {
                    return Err(DeskActError(
                        "Primary model requested clarification for an ID outside the supplied catalog candidates".into(),
                    ));
                }
                if !seen.insert(id) {
                    return Err(DeskActError(
                        "Primary model returned a duplicate app candidate ID".into(),
                    ));
                }
            }
            Ok(AppSelectionOutcome::Clarify(candidate_ids))
        }
    }
}

/// Add a custom executable entry after local target and name validation.
pub fn add_user_app_entry(
    name: &str,
    target: &str,
    aliases: Vec<String>,
) -> Result<AppCatalogEntry, DeskActError> {
    let name = validate_display_name(name)?;
    let target = validate_executable_target(target)?;
    let aliases = normalize_aliases(aliases)?;
    let _lock = acquire_catalog_lock()?;
    let mut store = read_store()?;
    let before = load_app_catalog()?;
    let candidate = AppCatalogEntry {
        id: new_user_entry_id(),
        name,
        target,
        arguments: String::new(),
        working_directory: String::new(),
        source: AppCatalogSource::UserConfigured,
        aliases,
    };
    let mut after = before.clone();
    after.push(candidate.clone());
    reject_new_conflicts(&before, &after)?;
    store.entries.push(candidate.clone());
    write_store(&store.entries)?;
    Ok(candidate)
}

/// Edit a custom entry without changing its stable ID; `aliases: None` preserves existing aliases.
pub fn edit_user_app_entry(
    id: &str,
    name: Option<&str>,
    target: Option<&str>,
    aliases: Option<Vec<String>>,
) -> Result<AppCatalogEntry, DeskActError> {
    let _lock = acquire_catalog_lock()?;
    let mut store = read_store()?;
    let mut catalog = load_app_catalog()?;
    let Some(index) = store.entries.iter().position(|entry| entry.id == id) else {
        return Err(DeskActError(format!(
            "No user-configured app entry has ID `{id}`"
        )));
    };
    let mut edited = store.entries[index].clone();
    if let Some(name) = name {
        edited.name = validate_display_name(name)?;
    }
    if let Some(target) = target {
        edited.target = validate_executable_target(target)?;
    }
    if let Some(aliases) = aliases {
        edited.aliases = normalize_aliases(aliases)?;
    }
    let before = catalog.clone();
    let Some(catalog_index) = catalog.iter().position(|entry| entry.id == id) else {
        return Err(DeskActError(format!(
            "No local catalog entry has ID `{id}`"
        )));
    };
    catalog[catalog_index] = edited.clone();
    reject_new_conflicts(&before, &catalog)?;
    store.entries[index] = edited.clone();
    write_store(&store.entries)?;
    Ok(edited)
}

/// Convert a local app entry into the model-safe candidate representation.
fn candidate(entry: &AppCatalogEntry) -> AppCandidate {
    AppCandidate {
        id: entry.id.clone(),
        display_name: entry.name.clone(),
    }
}

/// Normalize a name or alias for deterministic, case-insensitive matching.
fn app_name_key(value: &str) -> String {
    value.trim().to_lowercase()
}

/// Validate and trim a display name or alias.
fn validate_display_name(value: &str) -> Result<String, DeskActError> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(DeskActError(
            "App display name must be non-empty and contain no control characters".into(),
        ));
    }
    Ok(value.to_owned())
}

/// Validate aliases and reject duplicates under the app name matching rules.
fn normalize_aliases(aliases: Vec<String>) -> Result<Vec<String>, DeskActError> {
    let mut normalized = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for alias in aliases {
        let alias = validate_display_name(&alias)?;
        if !seen.insert(app_name_key(&alias)) {
            return Err(DeskActError(format!(
                "Duplicate app alias `{alias}` was provided"
            )));
        }
        normalized.push(alias);
    }
    Ok(normalized)
}

/// Require a custom target to be an existing absolute `.exe` file.
fn validate_executable_target(value: &str) -> Result<String, DeskActError> {
    let target = value.trim();
    let path = Path::new(target);
    if target.is_empty()
        || !path.is_absolute()
        || !path.is_file()
        || !path
            .extension()
            .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("exe"))
    {
        return Err(DeskActError(
            "Custom app target must be an existing .exe file at an absolute path; no app entry was saved"
                .into(),
        ));
    }
    Ok(target.to_owned())
}

/// Reject only name/alias conflicts introduced by the proposed catalog change.
fn reject_new_conflicts(
    before: &[AppCatalogEntry],
    after: &[AppCatalogEntry],
) -> Result<(), DeskActError> {
    let existing = catalog_conflicts(before);
    let after_conflicts = catalog_conflicts(after);
    if let Some((_, _, term)) = after_conflicts.difference(&existing).min() {
        return Err(DeskActError(format!(
            "App name or alias `{term}` introduces catalog conflicts"
        )));
    }
    Ok(())
}

/// Return every pairwise collision between canonical names and configured aliases.
fn catalog_conflicts(
    entries: &[AppCatalogEntry],
) -> std::collections::HashSet<(String, String, String)> {
    let mut conflicts = std::collections::HashSet::new();
    for (index, left) in entries.iter().enumerate() {
        let left_terms: std::collections::HashSet<_> = std::iter::once(left.name.as_str())
            .chain(left.aliases.iter().map(String::as_str))
            .map(app_name_key)
            .collect();
        for right in &entries[index + 1..] {
            let right_terms: std::collections::HashSet<_> = std::iter::once(right.name.as_str())
                .chain(right.aliases.iter().map(String::as_str))
                .map(app_name_key)
                .collect();
            let (first_id, second_id) = if left.id <= right.id {
                (left.id.clone(), right.id.clone())
            } else {
                (right.id.clone(), left.id.clone())
            };
            for term in left_terms.intersection(&right_terms) {
                conflicts.insert((first_id.clone(), second_id.clone(), term.clone()));
            }
        }
    }
    conflicts
}

/// Create a stable identity from app metadata, preserving case-sensitive shortcut arguments.
fn discovered_entry_id(app: &DiscoveredApp) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for (field, case_sensitive) in [
        (app.name.as_str(), false),
        (app.target.as_str(), false),
        (app.arguments.as_str(), true),
        (app.working_directory.as_str(), false),
    ] {
        let normalized = if case_sensitive {
            field.to_owned()
        } else {
            field.to_lowercase()
        };
        for byte in normalized.bytes().chain([0xff]) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("discovered-{hash:016x}")
}

/// Create a process-unique stable identifier for a newly configured entry.
fn new_user_entry_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = USER_ENTRY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!(
        "custom-{:x}-{:x}-{:x}",
        std::process::id(),
        timestamp,
        sequence
    )
}

/// Resolve the local settings path, honoring the developer/test override.
fn catalog_file_path() -> Result<PathBuf, DeskActError> {
    if let Some(path) = env::var_os("DESKACT_APP_CATALOG") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(windows)]
    let root = env::var_os("LOCALAPPDATA").map(PathBuf::from);
    #[cfg(not(windows))]
    let root = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    root.map(|root| root.join("DeskAct").join("apps.json"))
        .ok_or_else(|| {
            DeskActError("Could not locate a local DeskAct app-catalog settings directory".into())
        })
}

/// Acquire an exclusive OS file lock to serialize settings transactions across processes.
fn acquire_catalog_lock() -> Result<CatalogLock, DeskActError> {
    let catalog_path = catalog_file_path()?;
    let parent = settings_parent(&catalog_path);
    fs::create_dir_all(parent).map_err(|error| {
        DeskActError(format!(
            "Could not create app catalog settings directory `{}`: {error}",
            parent.display()
        ))
    })?;
    let name = catalog_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let lock_path = parent.join(format!("{name}.lock"));
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| {
            DeskActError(format!(
                "Could not open app catalog lock `{}`: {error}",
                lock_path.display()
            ))
        })?;
    FileExt::lock_exclusive(&file).map_err(|error| {
        DeskActError(format!(
            "Could not acquire app catalog lock `{}`: {error}",
            lock_path.display()
        ))
    })?;
    Ok(CatalogLock { file })
}

/// Return the settings directory, using the current directory for a bare filename override.
fn settings_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
}

/// Read the configured entries from the versioned local settings file.
fn read_user_entries() -> Result<Vec<AppCatalogEntry>, DeskActError> {
    Ok(read_store()?.entries)
}

/// Read and validate the versioned app catalog from disk.
fn read_store() -> Result<AppCatalogStore, DeskActError> {
    let path = catalog_file_path()?;
    let contents = match fs::read(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AppCatalogStore {
                version: STORE_VERSION,
                entries: Vec::new(),
            });
        }
        Err(error) => {
            return Err(DeskActError(format!(
                "Could not read app catalog `{}`: {error}",
                path.display()
            )));
        }
    };
    let store: AppCatalogStore = serde_json::from_slice(&contents).map_err(|error| {
        DeskActError(format!(
            "App catalog `{}` is invalid JSON or has an unsupported shape: {error}",
            path.display()
        ))
    })?;
    if store.version != STORE_VERSION {
        return Err(DeskActError(format!(
            "App catalog version {} is not supported (expected {STORE_VERSION})",
            store.version
        )));
    }
    ensure_unique_catalog_ids(&store.entries)?;
    for entry in &store.entries {
        if entry.source != AppCatalogSource::UserConfigured
            || validate_display_name(&entry.name)? != entry.name
            || normalize_aliases(entry.aliases.clone())? != entry.aliases
            || !Path::new(&entry.target).is_absolute()
            || !Path::new(&entry.target)
                .extension()
                .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("exe"))
            || !entry.arguments.is_empty()
            || !entry.working_directory.is_empty()
        {
            return Err(DeskActError(format!(
                "User app entry `{}` violates the local catalog constraints",
                entry.id
            )));
        }
    }
    Ok(store)
}

/// Atomically write an updated store while preserving the previous file on replacement failure.
fn write_store(entries: &[AppCatalogEntry]) -> Result<(), DeskActError> {
    let path = catalog_file_path()?;
    let parent = settings_parent(&path);
    fs::create_dir_all(parent).map_err(|error| {
        DeskActError(format!(
            "Could not create app catalog settings directory `{}`: {error}",
            parent.display()
        ))
    })?;
    let contents = serde_json::to_vec_pretty(&AppCatalogStore {
        version: STORE_VERSION,
        entries: entries.to_vec(),
    })
    .map_err(|error| DeskActError(format!("Could not serialize app catalog: {error}")))?;
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temp, contents).map_err(|error| {
        DeskActError(format!(
            "Could not write temporary app catalog `{}`: {error}",
            temp.display()
        ))
    })?;
    if let Err(error) = replace_catalog_file(&temp, &path) {
        let _ = fs::remove_file(&temp);
        return Err(DeskActError(format!(
            "Could not save app catalog `{}`; the existing catalog was preserved: {error}",
            path.display()
        )));
    }
    Ok(())
}

/// Replace the destination using the platform's atomic same-volume rename semantics.
#[cfg(not(windows))]
fn replace_catalog_file(temp: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(temp, destination)
}

/// Replace an existing Windows catalog atomically without deleting it before success.
#[cfg(windows)]
fn replace_catalog_file(temp: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};

    if !destination.exists() {
        return fs::rename(temp, destination);
    }

    #[link(name = "Kernel32")]
    unsafe extern "system" {
        fn ReplaceFileW(
            replaced_file_name: *const u16,
            replacement_file_name: *const u16,
            backup_file_name: *const u16,
            replace_flags: u32,
            exclude: *mut c_void,
            reserved: *mut c_void,
        ) -> i32;
    }

    let destination_wide: Vec<u16> = destination.as_os_str().encode_wide().chain([0]).collect();
    let temp_wide: Vec<u16> = temp.as_os_str().encode_wide().chain([0]).collect();
    // ReplaceFileW leaves the destination intact if the replacement fails.
    let succeeded = unsafe {
        ReplaceFileW(
            destination_wide.as_ptr(),
            temp_wide.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if succeeded == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Reject a catalog containing duplicate IDs.
fn ensure_unique_catalog_ids(entries: &[AppCatalogEntry]) -> Result<(), DeskActError> {
    let mut ids = std::collections::HashSet::new();
    if entries.iter().any(|entry| !ids.insert(entry.id.as_str())) {
        return Err(DeskActError(
            "App catalog contains a duplicate entry ID; no selection is safe".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A failed platform replacement must leave the only existing catalog untouched.
    #[test]
    fn failed_replacement_preserves_existing_catalog_file() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = env::temp_dir().join(format!(
            "deskact-replace-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let destination = dir.join("apps.json");
        let missing_temp = dir.join("missing.tmp");
        fs::write(&destination, b"old catalog data").unwrap();

        assert!(replace_catalog_file(&missing_temp, &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"old catalog data");
        fs::remove_dir_all(dir).unwrap();
    }
}
