#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]

#[cfg(not(windows))]
compile_error!("This throwaway probe is Windows-only.");

use std::{
    ffi::c_void,
    mem::zeroed,
    sync::atomic::{AtomicIsize, Ordering},
};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CreatePen, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
        EndPaint, FillRect, GetStockObject, NULL_BRUSH, PAINTSTRUCT, PS_SOLID, RoundRect,
        SelectObject, SetBkMode, SetTextColor, SetWindowRgn, TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{VK_ESCAPE, VK_F1},
        WindowsAndMessaging::{
            CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
            DispatchMessageW, GWL_STYLE, GWLP_USERDATA, GetMessageW, GetSystemMetrics,
            GetWindowLongPtrW, GetWindowRect, HTTRANSPARENT, IDC_ARROW, LWA_COLORKEY, LoadCursorW,
            MB_ICONERROR, MB_OK, MSG, MessageBoxW, PostQuitMessage, RegisterClassW, SM_CXSCREEN,
            SM_CYSCREEN, SW_HIDE, SW_SHOW, SW_SHOWNA, SWP_FRAMECHANGED, SWP_NOMOVE,
            SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow,
            SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow,
            TranslateMessage, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_NCCREATE,
            WM_NCDESTROY, WM_NCHITTEST, WM_PAINT, WNDCLASSW, WS_CAPTION, WS_EX_LAYERED,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX,
            WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
        },
    },
};

const ISLAND_WIDTH: i32 = 520;
const ISLAND_HEIGHT: i32 = 112;
const SETTINGS_WIDTH: i32 = 480;
const SETTINGS_HEIGHT: i32 = 320;
const TRANSPARENT_KEY: u32 = 0x00ff00ff;
const WHITE: u32 = 0x00ffffff;

static ISLAND: AtomicIsize = AtomicIsize::new(0);
static SETTINGS: AtomicIsize = AtomicIsize::new(0);

struct Surface {
    overlay: HWND,
    acrylic_state: *mut c_void,
}

struct OverlayText {
    heading: Vec<u16>,
    instructions: Vec<u16>,
    radius: i32,
}

#[cfg_attr(target_env = "msvc", link(name = "DeskActAcrylicShim"))]
unsafe extern "C" {
    fn deskact_bootstrap_initialize() -> i32;
    fn deskact_acrylic_attach(hwnd: HWND, output_state: *mut *mut c_void) -> i32;
    fn deskact_acrylic_detach(state: *mut c_void);
    fn deskact_composition_shutdown();
    fn deskact_bootstrap_shutdown();
}

fn main() {
    unsafe {
        check_hresult(
            deskact_bootstrap_initialize(),
            "Windows App SDK bootstrap initialization failed",
        );

        let instance = GetModuleHandleW(std::ptr::null());
        if instance.is_null() {
            fail("GetModuleHandleW failed");
        }

        let class_name = wide("DeskAct.NativeIslandProbe");
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&class) == 0 {
            fail("RegisterClassW failed for the backdrop window");
        }

        let overlay_class_name = wide("DeskAct.NativeIslandOverlay");
        let overlay_class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(overlay_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: std::ptr::null_mut(),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: overlay_class_name.as_ptr(),
        };
        if RegisterClassW(&overlay_class) == 0 {
            fail("RegisterClassW failed for the diagnostic overlay");
        }

        let island = create_surface(
            instance,
            &class_name,
            &overlay_class_name,
            ISLAND_WIDTH,
            ISLAND_HEIGHT,
            ISLAND_HEIGHT / 2,
            "DeskAct · Native island probe",
            "F1 opens separate Settings. Click away to check Acrylic. Esc quits.",
            true,
        );
        ISLAND.store(island as isize, Ordering::Relaxed);

        let settings = create_surface(
            instance,
            &class_name,
            &overlay_class_name,
            SETTINGS_WIDTH,
            SETTINGS_HEIGHT,
            24,
            "Settings · Independent native window",
            "Separate HWND with its own acrylic material. Esc returns to island.",
            false,
        );
        SETTINGS.store(settings as isize, Ordering::Relaxed);

        ShowWindow(island, SW_SHOW);
        ShowWindow(surface(&ISLAND).overlay, SW_SHOWNA);
        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }

        deskact_composition_shutdown();
        deskact_bootstrap_shutdown();
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_surface(
    instance: HINSTANCE,
    class_name: &[u16],
    overlay_class_name: &[u16],
    width: i32,
    height: i32,
    radius: i32,
    heading: &str,
    instructions: &str,
    centered: bool,
) -> HWND {
    let title = [0u16];
    let (x, y) = if centered {
        (
            (GetSystemMetrics(SM_CXSCREEN) - width) / 2,
            (GetSystemMetrics(SM_CYSCREEN) - height) / 2,
        )
    } else {
        (
            (GetSystemMetrics(SM_CXSCREEN) - width) / 2,
            (GetSystemMetrics(SM_CYSCREEN) - height) / 2 + 72,
        )
    };
    let surface = Box::into_raw(Box::new(Surface {
        overlay: std::ptr::null_mut(),
        acrylic_state: std::ptr::null_mut(),
    }));
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        class_name.as_ptr(),
        title.as_ptr(),
        WS_POPUP,
        x,
        y,
        width,
        height,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        instance,
        surface.cast(),
    );
    if hwnd.is_null() {
        drop(Box::from_raw(surface));
        fail("CreateWindowExW failed for the backdrop window");
    }

    enforce_chromeless_style(hwnd);
    apply_rounded_region(hwnd, radius);
    check_hresult(
        deskact_acrylic_attach(hwnd, &mut (*surface).acrylic_state),
        "DesktopAcrylicController could not attach to the native window",
    );
    (*surface).overlay = create_overlay(
        instance,
        overlay_class_name,
        hwnd,
        width,
        height,
        radius,
        heading,
        instructions,
    );
    hwnd
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_overlay(
    instance: HINSTANCE,
    class_name: &[u16],
    owner: HWND,
    width: i32,
    height: i32,
    radius: i32,
    heading: &str,
    instructions: &str,
) -> HWND {
    let mut rect: RECT = zeroed();
    if GetWindowRect(owner, &mut rect) == 0 {
        fail("GetWindowRect failed for the diagnostic overlay");
    }
    let title = [0u16];
    let text = Box::into_raw(Box::new(OverlayText {
        heading: wide(heading),
        instructions: wide(instructions),
        radius,
    }));
    let overlay = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
        class_name.as_ptr(),
        title.as_ptr(),
        WS_POPUP,
        rect.left,
        rect.top,
        width,
        height,
        owner,
        std::ptr::null_mut(),
        instance,
        text.cast(),
    );
    if overlay.is_null() {
        drop(Box::from_raw(text));
        fail("CreateWindowExW failed for the diagnostic overlay");
    }
    if SetLayeredWindowAttributes(overlay, TRANSPARENT_KEY, 0, LWA_COLORKEY) == 0 {
        fail("SetLayeredWindowAttributes failed for the diagnostic overlay");
    }
    overlay
}

unsafe fn enforce_chromeless_style(hwnd: HWND) {
    let mut style = GetWindowLongPtrW(hwnd, GWL_STYLE);
    style &= !((WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX) as isize);
    SetWindowLongPtrW(hwnd, GWL_STYLE, style);
    let _ = SetWindowPos(
        hwnd,
        std::ptr::null_mut(),
        0,
        0,
        0,
        0,
        SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER,
    );
}

unsafe fn apply_rounded_region(hwnd: HWND, radius: i32) {
    let mut rect: RECT = zeroed();
    if GetWindowRect(hwnd, &mut rect) == 0 {
        return;
    }
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let diameter = (radius * 2).min(width.min(height));
    let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, diameter, diameter);
    if !region.is_null() && SetWindowRgn(hwnd, region, 1) == 0 {
        let _ = DeleteObject(region);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCREATE => {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            1
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = zeroed();
            BeginPaint(hwnd, &mut paint);
            EndPaint(hwnd, &paint);
            0
        }
        WM_KEYDOWN if wparam as u32 == VK_F1 as u32 && hwnd == handle(&ISLAND) => {
            let settings = handle(&SETTINGS);
            ShowWindow(settings, SW_SHOW);
            ShowWindow(surface(&SETTINGS).overlay, SW_SHOWNA);
            let _ = SetForegroundWindow(settings);
            0
        }
        WM_KEYDOWN if wparam as u32 == VK_ESCAPE as u32 && hwnd == handle(&SETTINGS) => {
            ShowWindow(surface(&SETTINGS).overlay, SW_HIDE);
            ShowWindow(hwnd, SW_HIDE);
            let island = handle(&ISLAND);
            let _ = SetForegroundWindow(island);
            0
        }
        WM_KEYDOWN if wparam as u32 == VK_ESCAPE as u32 && hwnd == handle(&ISLAND) => {
            DestroyWindow(hwnd);
            0
        }
        WM_CLOSE if hwnd == handle(&SETTINGS) => {
            ShowWindow(surface(&SETTINGS).overlay, SW_HIDE);
            ShowWindow(hwnd, SW_HIDE);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY if hwnd == handle(&ISLAND) => {
            let settings = handle(&SETTINGS);
            if !settings.is_null() {
                DestroyWindow(settings);
            }
            PostQuitMessage(0);
            0
        }
        WM_NCDESTROY => {
            let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Surface;
            if !data.is_null() {
                if !(*data).overlay.is_null() {
                    DestroyWindow((*data).overlay);
                }
                if !(*data).acrylic_state.is_null() {
                    deskact_acrylic_detach((*data).acrylic_state);
                }
                drop(Box::from_raw(data));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_NCCREATE => {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
            1
        }
        WM_NCHITTEST => HTTRANSPARENT as LRESULT,
        WM_ERASEBKGND => {
            fill_transparency_key(wparam as _, hwnd);
            1
        }
        WM_PAINT => {
            let mut paint: PAINTSTRUCT = zeroed();
            let dc = BeginPaint(hwnd, &mut paint);
            if !dc.is_null() {
                fill_transparency_key(dc, hwnd);
                let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayText;
                if !data.is_null() {
                    draw_overlay(dc, &*data, hwnd);
                }
            }
            EndPaint(hwnd, &paint);
            0
        }
        WM_NCDESTROY => {
            let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayText;
            if !data.is_null() {
                drop(Box::from_raw(data));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn fill_transparency_key(dc: windows_sys::Win32::Graphics::Gdi::HDC, hwnd: HWND) {
    let mut rect: RECT = zeroed();
    let _ = windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rect);
    let brush = CreateSolidBrush(TRANSPARENT_KEY);
    if !brush.is_null() {
        FillRect(dc, &rect, brush);
        let _ = DeleteObject(brush);
    }
}

unsafe fn draw_overlay(dc: windows_sys::Win32::Graphics::Gdi::HDC, text: &OverlayText, hwnd: HWND) {
    let mut client: RECT = zeroed();
    let _ = windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client);
    SetBkMode(dc, TRANSPARENT as i32);
    SetTextColor(dc, WHITE);

    let previous_font = SelectObject(
        dc,
        GetStockObject(windows_sys::Win32::Graphics::Gdi::DEFAULT_GUI_FONT),
    );
    let heading = &text.heading[..text.heading.len().saturating_sub(1)];
    let instructions = &text.instructions[..text.instructions.len().saturating_sub(1)];
    let mut heading_rect = RECT {
        left: 28,
        top: 18,
        right: client.right - 24,
        bottom: 54,
    };
    DrawTextW(
        dc,
        heading.as_ptr(),
        heading.len() as i32,
        &mut heading_rect,
        0x00000001 | 0x00000020,
    );
    let mut instructions_rect = RECT {
        left: 28,
        top: 58,
        right: client.right - 24,
        bottom: 96,
    };
    DrawTextW(
        dc,
        instructions.as_ptr(),
        instructions.len() as i32,
        &mut instructions_rect,
        0x00000001 | 0x00000020,
    );

    let pen = CreatePen(PS_SOLID, 1, WHITE);
    if !pen.is_null() {
        let previous_pen = SelectObject(dc, pen);
        let previous_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
        let _ = RoundRect(
            dc,
            1,
            1,
            client.right - 1,
            client.bottom - 1,
            text.radius * 2,
            text.radius * 2,
        );
        SelectObject(dc, previous_brush);
        SelectObject(dc, previous_pen);
        let _ = DeleteObject(pen);
    }
    SelectObject(dc, previous_font);
}

fn surface(value: &AtomicIsize) -> &'static mut Surface {
    let hwnd = handle(value);
    unsafe {
        let pointer = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Surface;
        if pointer.is_null() {
            fail("native surface state was not initialized");
        }
        &mut *pointer
    }
}

fn handle(value: &AtomicIsize) -> HWND {
    value.load(Ordering::Relaxed) as HWND
}

fn check_hresult(result: i32, message: &str) {
    if result < 0 {
        fail(&format!("{message} (HRESULT 0x{:08X})", result as u32));
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn fail(message: &str) -> ! {
    eprintln!("native island probe: {message}");
    let title = wide("Native Island Probe");
    let message = wide(message);
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            message.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
    std::process::exit(1);
}
