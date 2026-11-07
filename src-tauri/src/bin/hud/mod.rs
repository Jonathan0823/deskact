mod shortcuts;
mod tray;
mod window_behavior;

use shortcuts::{ShortcutState, ShortcutStatus};

pub fn run() {
    tauri::Builder::default()
        .manage(ShortcutState::default())
        .manage(window_behavior::PreviousForegroundWindow::default())
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
fn dismiss_island(app: tauri::AppHandle) -> Result<(), String> {
    window_behavior::dismiss(&app)
}
