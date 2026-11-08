use std::{
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewWindow};

#[derive(Default)]
pub struct WindowMorphGeneration(AtomicU64);

pub const INVOKED_EVENT: &str = "deskact://invoked";
pub const SETTINGS_EVENT: &str = "deskact://open-settings";
pub const QUIT_REQUESTED_EVENT: &str = "deskact://quit-requested";

#[derive(Default)]
pub struct PreviousForegroundWindow(Mutex<Option<isize>>);

pub fn open_composer(app: &AppHandle) {
    open_window(app, INVOKED_EVENT);
}

pub fn open_settings(app: &AppHandle) {
    open_window(app, SETTINGS_EVENT);
}

pub fn request_quit(app: &AppHandle) {
    if app.state::<super::tasks::TaskHub>().has_active_task() {
        open_window(app, QUIT_REQUESTED_EVENT);
    } else {
        app.exit(0);
    }
}

pub fn quit_application(app: &AppHandle) {
    app.exit(0);
}

fn open_window(app: &AppHandle, event: &str) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    if !window.is_focused().unwrap_or(false)
        && let Some(hwnd) = foreground_window()
    {
        *app.state::<PreviousForegroundWindow>()
            .0
            .lock()
            .expect("foreground-window state poisoned") = Some(hwnd);
        position_on_foreground_monitor(&window, hwnd);
    }

    if window.show().is_ok() && window.set_focus().is_ok() {
        let _ = window.emit(event, ());
    }
}

pub fn yield_focus(app: &AppHandle) {
    restore_foreground(app);
}

pub async fn morph_window(
    app: &AppHandle,
    width: f64,
    height: f64,
    corner_radius: f64,
    reduced_motion: bool,
) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "DeskAct's window is unavailable".to_owned())?;
    let generation = app
        .state::<WindowMorphGeneration>()
        .0
        .fetch_add(1, Ordering::Relaxed)
        + 1;
    let target_width = width.clamp(280.0, 560.0);
    let target_height = height.clamp(48.0, 480.0);
    let scale = window
        .scale_factor()
        .map_err(|_| "Could not read DeskAct's display scale".to_owned())?;
    let target = PhysicalSize::new(
        (target_width * scale).round() as u32,
        (target_height * scale).round() as u32,
    );
    let start = window
        .inner_size()
        .map_err(|_| "Could not read DeskAct's current size".to_owned())?;
    let position = window
        .outer_position()
        .map_err(|_| "Could not read DeskAct's current position".to_owned())?;
    let center_x = position.x + (start.width as i32 / 2);
    let top = position.y;
    // Set the shape first; the HWND resize hook keeps it in sync at every frame.
    update_rounded_window_region(&window, corner_radius, scale)?;
    if start == target {
        return Ok(());
    }
    if reduced_motion {
        window
            .set_size(target)
            .map_err(|_| "Could not resize DeskAct".to_owned())?;
        window
            .set_position(PhysicalPosition::new(
                center_x - target.width as i32 / 2,
                top,
            ))
            .map_err(|_| "Could not anchor DeskAct after resize".to_owned())?;
        return Ok(());
    }

    const FRAMES: u32 = 14;
    for frame in 1..=FRAMES {
        if app
            .state::<WindowMorphGeneration>()
            .0
            .load(Ordering::Relaxed)
            != generation
        {
            return Ok(());
        }
        let progress = frame as f64 / FRAMES as f64;
        let eased = 1.0 - (1.0 - progress).powi(3);
        let width = (start.width as f64 + (target.width as f64 - start.width as f64) * eased)
            .round() as u32;
        let height = (start.height as f64 + (target.height as f64 - start.height as f64) * eased)
            .round() as u32;
        window
            .set_size(PhysicalSize::new(width, height))
            .map_err(|_| "Could not morph DeskAct's size".to_owned())?;
        window
            .set_position(PhysicalPosition::new(center_x - width as i32 / 2, top))
            .map_err(|_| "Could not anchor DeskAct during resize".to_owned())?;
        tokio::time::sleep(Duration::from_millis(16)).await;
    }
    if app
        .state::<WindowMorphGeneration>()
        .0
        .load(Ordering::Relaxed)
        == generation
    {
        window
            .set_size(target)
            .map_err(|_| "Could not finish resizing DeskAct".to_owned())?;
        window
            .set_position(PhysicalPosition::new(
                center_x - target.width as i32 / 2,
                top,
            ))
            .map_err(|_| "Could not anchor DeskAct after resize".to_owned())?;
    }
    Ok(())
}

pub fn dismiss(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "DeskAct's window is unavailable".to_owned())?;
    window
        .hide()
        .map_err(|_| "Could not hide the DeskAct window".to_owned())?;
    restore_foreground(app);
    Ok(())
}

fn restore_foreground(app: &AppHandle) {
    let previous = app
        .state::<PreviousForegroundWindow>()
        .0
        .lock()
        .expect("foreground-window state poisoned")
        .take();
    if let Some(hwnd) = previous {
        restore_window(hwnd);
    }
}

#[cfg(windows)]
fn foreground_window() -> Option<isize> {
    let hwnd = unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
    (!hwnd.is_null()).then_some(hwnd as isize)
}

#[cfg(not(windows))]
fn foreground_window() -> Option<isize> {
    None
}

#[cfg(windows)]
fn restore_window(hwnd: isize) {
    unsafe {
        let _ = windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd as _);
    }
}

#[cfg(not(windows))]
fn restore_window(_hwnd: isize) {}

#[cfg(windows)]
fn position_on_foreground_monitor(window: &WebviewWindow, foreground: isize) {
    use std::mem::size_of;
    use tauri::PhysicalPosition;
    use windows_sys::Win32::{
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::GetForegroundWindow},
    };

    let foreground_hwnd = if foreground == 0 {
        unsafe { GetForegroundWindow() }
    } else {
        foreground as _
    };
    if foreground_hwnd.is_null() {
        return;
    }
    let monitor = unsafe { MonitorFromWindow(foreground_hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_null() {
        return;
    }

    let mut info = unsafe { std::mem::zeroed::<MONITORINFO>() };
    info.cbSize = size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(monitor, &mut info) } == 0 {
        return;
    }
    let Ok(window_size) = window.outer_size() else {
        return;
    };
    let scale = (unsafe { GetDpiForWindow(foreground_hwnd) } as f64 / 96.0).max(1.0);
    let monitor_rect = info.rcWork;
    let width = monitor_rect.right.saturating_sub(monitor_rect.left).max(0) as u32;
    let x = monitor_rect.left + ((width.saturating_sub(window_size.width) / 2) as i32).max(0);
    let y = monitor_rect.top + (16.0 * scale).round() as i32;
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

#[cfg(not(windows))]
fn position_on_foreground_monitor(_window: &WebviewWindow, _foreground: isize) {}

#[cfg(windows)]
fn update_rounded_window_region(
    window: &WebviewWindow,
    corner_radius: f64,
    scale: f64,
) -> Result<(), String> {
    let hwnd = window
        .hwnd()
        .map_err(|_| "Could not access DeskAct's native window handle".to_owned())?;
    let radius = (corner_radius.max(0.0) * scale).round() as usize;
    // SendMessage synchronizes with the guard on the native event-loop thread.
    unsafe { super::windows_frame::set_corner_radius(hwnd.0, radius) }
        .map_err(|error| format!("Could not update DeskAct's rounded window region: {error}"))
}

#[cfg(not(windows))]
fn update_rounded_window_region(
    _window: &WebviewWindow,
    _corner_radius: f64,
    _scale: f64,
) -> Result<(), String> {
    Ok(())
}
