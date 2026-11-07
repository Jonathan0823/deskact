#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{Manager, PhysicalPosition};

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let Some(window) = app.get_webview_window("main") else {
                return Err(std::io::Error::other("The Adaptive Island window is missing").into());
            };

            if let Some(monitor) = window.current_monitor()? {
                let monitor_position = monitor.position();
                let monitor_size = monitor.size();
                let window_size = window.outer_size()?;
                let x = monitor_position.x
                    + ((monitor_size.width as i32 - window_size.width as i32) / 2).max(0);
                let y = monitor_position.y + (16.0 * monitor.scale_factor()) as i32;
                window.set_position(PhysicalPosition::new(x, y))?;
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running DeskAct");
}
