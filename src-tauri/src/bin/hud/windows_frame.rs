//! Keep the island's HWND frameless, including during native activation/theme changes.
//!
//! Tao leaves caption styles on undecorated top-level windows and removes the frame
//! through WM_NCCALCSIZE. DWM can still paint those captions on deactivation. Strip
//! the styles as well, and preserve Tao's focus events without non-client painting.

use std::{cell::Cell, io, mem::size_of, ptr::null_mut};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, RECT, SetLastError, WPARAM},
    Graphics::{
        Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_WINDOW_CORNER_PREFERENCE,
            DWMWCP_DONOTROUND, DwmSetWindowAttribute,
        },
        Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn},
    },
    UI::{
        HiDpi::GetDpiForWindow,
        Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{
            GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, GetWindowRect, HWND_TOP, STYLESTRUCT,
            SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageW,
            SetWindowLongW, SetWindowPos, WINDOWPOS, WM_APP, WM_NCACTIVATE, WM_NCCALCSIZE,
            WM_NCDESTROY, WM_NCPAINT, WM_STYLECHANGING, WM_WINDOWPOSCHANGING, WS_CAPTION,
            WS_EX_CLIENTEDGE, WS_EX_DLGMODALFRAME, WS_EX_STATICEDGE, WS_EX_WINDOWEDGE,
            WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU, WS_THICKFRAME,
        },
    },
};

const SUBCLASS_ID: usize = 0x44414354;
const SET_CORNER_RADIUS: u32 = WM_APP + 0x444;

thread_local! {
    // SetWindowRgn itself sends position messages. Avoid recursively setting it.
    static SHAPING_WINDOW: Cell<HWND> = const { Cell::new(null_mut()) };
}
const FRAME_STYLES: u32 = WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
const FRAME_EX_STYLES: u32 =
    WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_DLGMODALFRAME | WS_EX_STATICEDGE;

/// Install on the thread that owns `hwnd`, before its first show.
///
/// # Safety
/// `hwnd` must be a live island window owned by the calling thread.
pub unsafe fn install(hwnd: HWND) -> io::Result<()> {
    unsafe {
        let radius = (52.0 * f64::from(GetDpiForWindow(hwnd)) / 96.0).round() as usize;
        if SetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID, radius) == 0 {
            return Err(io::Error::other("Could not install the island frame guard"));
        }
        set_style(
            hwnd,
            GWL_STYLE,
            GetWindowLongW(hwnd, GWL_STYLE) as u32 & !FRAME_STYLES,
        )?;
        set_style(
            hwnd,
            GWL_EXSTYLE,
            GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & !FRAME_EX_STYLES,
        )?;
        if SetWindowPos(
            hwnd,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut rect: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return Err(io::Error::last_os_error());
        }
        apply_region(hwnd, rect.right - rect.left, rect.bottom - rect.top, radius)?;
        // The HRGN owns the custom capsule shape. DWM's standard corner hint is
        // inapplicable to a region-shaped HWND; do not ask both systems to round it.
        let corner = DWMWCP_DONOTROUND;
        let no_border = DWMWA_COLOR_NONE;
        // These attributes are optional on older Windows; frame suppression isn't.
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&corner as *const i32).cast(),
            size_of::<i32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            (&no_border as *const u32).cast(),
            size_of::<u32>() as u32,
        );
    }
    Ok(())
}

/// Update the native clip on its owning thread. The resize hook then reapplies it
/// for each new HWND size, before Windows paints a rectangular intermediate frame.
///
/// # Safety
/// `hwnd` must be a live window with this frame guard installed.
pub unsafe fn set_corner_radius(hwnd: HWND, radius: usize) -> io::Result<()> {
    if unsafe { SendMessageW(hwnd, SET_CORNER_RADIUS, radius, 0) } == 0 {
        return Err(io::Error::other(
            "Could not update the island window region",
        ));
    }
    Ok(())
}

unsafe fn apply_region(hwnd: HWND, width: i32, height: i32, radius: usize) -> io::Result<()> {
    if width <= 0 || height <= 0 {
        return Ok(());
    }
    if SHAPING_WINDOW.get() == hwnd {
        return Ok(());
    }
    let diameter = radius.saturating_mul(2).min(width.min(height) as usize) as i32;
    unsafe {
        // Region bounds are exclusive, just like the client/CSS surface bounds.
        let region = CreateRoundRectRgn(0, 0, width, height, diameter, diameter);
        if region.is_null() {
            return Err(io::Error::last_os_error());
        }
        let previous = SHAPING_WINDOW.replace(hwnd);
        let result = SetWindowRgn(hwnd, region, 0);
        SHAPING_WINDOW.set(previous);
        if result == 0 {
            DeleteObject(region);
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

unsafe fn set_style(hwnd: HWND, index: i32, value: u32) -> io::Result<()> {
    unsafe {
        SetLastError(0);
        if SetWindowLongW(hwnd, index, value as i32) == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                return Err(error);
            }
        }
    }
    Ok(())
}

unsafe extern "system" fn frame_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    radius: usize,
) -> LRESULT {
    unsafe {
        match message {
            SET_CORNER_RADIUS => {
                let mut rect: RECT = std::mem::zeroed();
                if SetWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID, wparam) == 0
                    || GetWindowRect(hwnd, &mut rect) == 0
                {
                    return 0;
                }
                return if apply_region(hwnd, rect.right - rect.left, rect.bottom - rect.top, wparam)
                    .is_ok()
                {
                    1
                } else {
                    0
                };
            }
            WM_WINDOWPOSCHANGING if lparam != 0 => {
                let position = &*(lparam as *const WINDOWPOS);
                if position.flags & SWP_NOSIZE == 0 {
                    // Shape and dimensions change in the same native message,
                    // not in a later async Tauri command after the window grew.
                    let _ = apply_region(hwnd, position.cx, position.cy, radius);
                }
            }
            WM_STYLECHANGING if lparam != 0 => {
                let styles = &mut *(lparam as *mut STYLESTRUCT);
                if wparam as i32 == GWL_STYLE {
                    styles.styleNew &= !FRAME_STYLES;
                } else if wparam as i32 == GWL_EXSTYLE {
                    styles.styleNew &= !FRAME_EX_STYLES;
                }
            }
            // Keep Tao's activation/focus bookkeeping; -1 tells DefWindowProc
            // not to repaint a native caption as focus moves to another app.
            WM_NCACTIVATE => return DefSubclassProc(hwnd, message, wparam, -1),
            WM_NCCALCSIZE | WM_NCPAINT => return 0,
            WM_NCDESTROY => {
                RemoveWindowSubclass(hwnd, Some(frame_proc), SUBCLASS_ID);
            }
            _ => {}
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, ptr::null_mut};
    use windows_sys::{
        Win32::{
            Foundation::RECT,
            Graphics::Gdi::{GetWindowRgn, PtInRegion},
            System::LibraryLoader::GetModuleHandleW,
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowRect,
                RegisterClassW, SendMessageW, UnregisterClassW, WNDCLASSW, WS_OVERLAPPEDWINDOW,
            },
        },
        core::w,
    };

    thread_local! {
        static ACTIVATION: Cell<Option<(WPARAM, LPARAM)>> = const { Cell::new(None) };
        static NATIVE_PAINTS: Cell<u32> = const { Cell::new(0) };
    }

    unsafe extern "system" fn test_proc(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if message == WM_NCACTIVATE {
            ACTIVATION.set(Some((wparam, lparam)));
        } else if message == WM_NCPAINT {
            NATIVE_PAINTS.set(NATIVE_PAINTS.get() + 1);
        }
        unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
    }

    #[test]
    fn native_frame_stays_absent_through_activation_and_style_changes() {
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class_name = w!("DeskActFrameRegression");
            let class = WNDCLASSW {
                hInstance: instance,
                lpszClassName: class_name,
                lpfnWndProc: Some(test_proc),
                ..std::mem::zeroed()
            };
            assert_ne!(RegisterClassW(&class), 0);
            let hwnd = CreateWindowExW(
                WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE,
                class_name,
                w!("Caption must never be drawn"),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                460,
                88,
                null_mut(),
                null_mut(),
                instance,
                std::ptr::null(),
            );
            assert!(!hwnd.is_null());
            install(hwnd).unwrap();
            assert_eq!(GetWindowLongW(hwnd, GWL_STYLE) as u32 & FRAME_STYLES, 0);
            assert_eq!(
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & FRAME_EX_STYLES,
                0
            );

            for active in [1, 0, 1, 0] {
                SendMessageW(hwnd, WM_NCACTIVATE, active, 0);
                assert_eq!(ACTIVATION.get(), Some((active, -1)));
            }
            let paints = NATIVE_PAINTS.get();
            SendMessageW(hwnd, WM_NCPAINT, 1, 0);
            assert_eq!(NATIVE_PAINTS.get(), paints);

            // Framework style updates must not reintroduce caption/resize borders.
            SetWindowLongW(hwnd, GWL_STYLE, WS_OVERLAPPEDWINDOW as i32);
            SetWindowLongW(hwnd, GWL_EXSTYLE, FRAME_EX_STYLES as i32);
            assert_eq!(GetWindowLongW(hwnd, GWL_STYLE) as u32 & FRAME_STYLES, 0);
            assert_eq!(
                GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & FRAME_EX_STYLES,
                0
            );
            assert_ne!(
                SetWindowPos(
                    hwnd,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
                ),
                0
            );
            let mut outer: RECT = std::mem::zeroed();
            let mut client: RECT = std::mem::zeroed();
            assert_ne!(GetWindowRect(hwnd, &mut outer), 0);
            assert_ne!(GetClientRect(hwnd, &mut client), 0);
            assert_eq!(outer.right - outer.left, client.right - client.left);
            assert_eq!(outer.bottom - outer.top, client.bottom - client.top);

            let region = CreateRoundRectRgn(0, 0, 460, 88, 104, 104);
            assert!(!region.is_null());
            assert!(GetWindowRgn(hwnd, region) > 0);
            assert_eq!(
                PtInRegion(region, 0, 0),
                0,
                "capsule corner must be clipped"
            );
            assert_ne!(
                PtInRegion(region, 230, 44),
                0,
                "capsule center must remain visible"
            );
            assert_eq!(
                PtInRegion(region, 459, 0),
                0,
                "opposite corner must be clipped"
            );
            DeleteObject(region);
            assert_ne!(DestroyWindow(hwnd), 0);
            assert_ne!(UnregisterClassW(class_name, instance), 0);
        }
    }
}
