//! Native CLI preflight, Windows application inventory, and controlled developer acceptance workflows for DeskAct.

use std::{fmt, time::Duration};

use serde::Deserialize;

mod approval;
pub use approval::{
    ActionAdapter, ActionRiskClass, ActionScope, ApprovalDecision, ApprovalGrant, ApprovalRequest,
    ApprovalSimulationReport, authorize_action, classify_action, explicitly_approve,
    request_approval, simulate_approval_scenario,
};
mod app_catalog;
pub use app_catalog::{
    AppCandidate, AppCatalogEntry, AppCatalogSource, AppSelectionInput, AppSelectionOutcome,
    ModelAppSelection, add_user_app_entry, edit_user_app_entry, load_app_catalog,
    prepare_app_selection, validate_model_app_selection,
};
mod llm;
pub use llm::{
    ActionPlan, ActionPlanStep, LlmConnectionReport, request_action_plan,
    request_action_plan_with_context, test_llm_connection, validate_action_plan_json,
};
mod execution;
pub use execution::{
    NativeActionAdapter, PreparedTask, TaskExecutionEvidence, TaskExecutionReport,
    execute_prepared_task, model_candidate_context, open_app_plan, registered_open_workflow,
    validate_open_app_request,
};
mod task;
pub use task::{
    ActionPlanner, CancellationToken, ChatCompletionsPlanner, DeliveryDisposition,
    ManualRestorePrompt, MutationLease, PostconditionState, RecoveryDecision, TaskCoordinator,
    TaskSimulationReport, may_resume_after_interference, prepare_task_request, recovery_decision,
    requires_fresh_observation, run_task, simulate_task_scenario,
};
mod windows_catalog;
pub use windows_catalog::{
    CatalogDiscovery, DiscoveredApp, DiscoverySource, discover_app_catalog, process_image_path,
};
mod native;
pub(crate) use native::{
    CommandRunError, NATIVE_PROCESS_TIMEOUT, normalize_catalog, run_command_with_timeout,
};
pub use native::{VerifiedNativeCli, native_inventory, preflight, resolve_native_target};
mod acceptance;
mod snapshot;
pub use acceptance::{
    NotepadAcceptanceReport, run_calculator_acceptance, run_file_explorer_acceptance,
    run_notepad_acceptance, run_vlc_acceptance,
};

/// Exact `agent-desktop` release required for native CLI operations.
pub const SUPPORTED_AGENT_DESKTOP_VERSION: &str = "1.1.0";
/// Structured JSON contract version required from `agent-desktop`.
pub const SUPPORTED_JSON_CONTRACT_VERSION: &str = "2.6";
/// Maximum time to wait after activation for the target window to become enumerable.
pub const APP_READINESS_TIMEOUT: Duration = Duration::from_secs(10);

/// Verified versions reported by the installed native automation CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightStatus {
    /// Exact installed `agent-desktop` version.
    pub tool_version: String,
    /// Structured JSON contract version reported by the CLI.
    pub contract_version: String,
}

/// A currently running GUI application reported by `agent-desktop`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct RunningApp {
    /// Application or process name.
    pub name: String,
    /// Operating-system process identifier.
    pub pid: u32,
    /// Whether the window is foreground or background, as reported by the CLI.
    pub presentation: String,
}

/// Read-only snapshot of discovered application entries and running GUI apps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeInventory {
    /// Verified native CLI versions.
    pub preflight: PreflightStatus,
    /// Deterministically ordered discovered app entries.
    pub catalog: Vec<DiscoveredApp>,
    /// Number of Start Menu shortcuts skipped because they lacked a file target.
    pub skipped_start_menu_shortcuts: usize,
    /// Currently running GUI applications.
    pub running_apps: Vec<RunningApp>,
}

/// A fresh, uniquely matched native window with an accessibility snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedNativeTarget {
    /// Discovered catalog entry selected by its exact display name (case-insensitive).
    pub app: DiscoveredApp,
    /// Window identity from the fresh `list-windows` response.
    pub window: NativeWindow,
    /// Identifier of the fresh complete accessibility snapshot.
    pub snapshot_id: String,
    /// Number of actionable accessibility references in the snapshot.
    pub ref_count: usize,
}

/// Window returned by `agent-desktop list-windows`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NativeWindow {
    /// Stable window identifier from the fresh window inventory.
    pub id: String,
    /// Visible window title reported by the native automation tool.
    pub title: String,
    /// Process/app label reported for the window; not proof of executable ownership.
    pub app_name: String,
    /// Whether the window exposes semantic accessibility controls.
    pub accessible: bool,
    /// Whether the window is currently minimized.
    pub minimized: bool,
    /// Whether the window currently has OS focus, when reported by the CLI.
    #[serde(default)]
    pub is_focused: bool,
    /// Whether the window is visible, when reported by the CLI.
    #[serde(default)]
    pub visible: bool,
    /// Current operating-system process id reported for the window.
    #[serde(default)]
    pub pid: u32,
}

/// Resolution may pause so the user can manually restore a minimized window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeTargetResolution {
    /// A uniquely matched, owner-verified accessible window has a complete snapshot.
    Ready(ResolvedNativeTarget),
    /// The user must restore the selected window manually before resolution can continue.
    NeedsManualRestore {
        /// The selected discovered app entry.
        app: DiscoveredApp,
        /// The minimized window to restore.
        window: NativeWindow,
    },
}

/// Error returned when native CLI preflight or inventory cannot be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeskActError(String);

impl DeskActError {
    /// Construct an error from an actionable diagnostic message.
    pub fn new(message: String) -> Self {
        Self(message)
    }
}

impl fmt::Display for DeskActError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for DeskActError {}
