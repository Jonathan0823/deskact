mod shortcuts;
mod tasks;
mod tray;
mod window_behavior;

use shortcuts::{ShortcutState, ShortcutStatus};
use tasks::{TaskHub, get_last_task_result, get_task_snapshot, submit_task};
use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .manage(ShortcutState::default())
        .manage(window_behavior::PreviousForegroundWindow::default())
        .manage(TaskHub::default())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _, event| {
                    if event.state == tauri_plugin_global_shortcut::ShortcutState::Pressed {
                        window_behavior::open_composer(app);
                    }
                })
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_shortcut_status,
            update_global_shortcut,
            get_task_snapshot,
            get_last_task_result,
            submit_task,
            yield_focus,
            dismiss_island
        ])
        .setup(|app| {
            let state = app.state::<ShortcutState>();
            shortcuts::install_shortcut(app.handle(), state.inner());
            tray::install(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running DeskAct");
}

#[tauri::command]
fn get_shortcut_status(state: tauri::State<'_, ShortcutState>) -> ShortcutStatus {
    state.snapshot()
}

#[tauri::command]
fn update_global_shortcut(
    app: tauri::AppHandle,
    state: tauri::State<'_, ShortcutState>,
    accelerator: String,
) -> Result<ShortcutStatus, String> {
    shortcuts::update_global_shortcut(&app, state.inner(), accelerator)
}

#[tauri::command]
fn yield_focus(app: tauri::AppHandle) {
    window_behavior::yield_focus(&app);
}

#[tauri::command]
fn dismiss_island(app: tauri::AppHandle) -> Result<(), String> {
    window_behavior::dismiss(&app)
}
