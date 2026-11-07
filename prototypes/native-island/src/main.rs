#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(unsafe_op_in_unsafe_fn)]

#[cfg(not(windows))]
compile_error!("This throwaway probe is Windows-only.");

use std::{
    mem::zeroed,
    sync::atomic::{AtomicIsize, Ordering},
};

use windows_sys::Win32::{
    Foundation::{BOOL, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::{
        Dwm::{DwmExtendFrameIntoClientArea, DwmSetWindowAttribute},
        Gdi::{BeginPaint, CreateRoundRectRgn, DeleteObject, EndPaint, PAINTSTRUCT, SetWindowRgn},
    },
    System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
    UI::{
        Controls::MARGINS,
        Input::KeyboardAndMouse::{VK_ESCAPE, VK_F1},
        WindowsAndMessaging::{
            CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow,
            DispatchMessageW, GWLP_USERDATA, GetClientRect, GetMessageW, GetSystemMetrics,
            GetWindowLongPtrW, GetWindowRect, IDC_ARROW, LoadCursorW, MSG, PostQuitMessage,
            RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE, SW_SHOW, SetForegroundWindow,
            SetWindowLongPtrW, ShowWindow, TranslateMessage, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND,
            WM_KEYDOWN, WM_NCCREATE, WM_NCDESTROY, WM_PAINT, WNDCLASSW, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_POPUP,
        },
    },
};

use windows::{
    Win32::{
        Foundation::HWND as D2dHwnd,
        Graphics::{
            Direct2D::{
                Common::{
                    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F,
                    D2D1_PIXEL_FORMAT,
                },
                D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
                D2D1_HWND_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_IMMEDIATELY,
                D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_ROUNDED_RECT,
                D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory, ID2D1Factory,
                ID2D1HwndRenderTarget,
            },
            DirectWrite::{
                DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT_NORMAL, DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
                DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
        },
    },
    core::w,
};

const ISLAND_WIDTH: i32 = 520;
const ISLAND_HEIGHT: i32 = 112;
const SETTINGS_WIDTH: i32 = 480;
const SETTINGS_HEIGHT: i32 = 320;
static ISLAND: AtomicIsize = AtomicIsize::new(0);
static SETTINGS: AtomicIsize = AtomicIsize::new(0);

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    tint: u32,
    animation_id: u32,
}

#[repr(C)]
struct WindowCompositionAttributeData {
    attribute: u32,
    data: *mut AccentPolicy,
    size: usize,
}

type SetWindowCompositionAttributeFn =
    unsafe extern "system" fn(HWND, *mut WindowCompositionAttributeData) -> BOOL;

struct SurfaceText {
    heading: Vec<u16>,
    instructions: Vec<u16>,
    radius: i32,
    renderer: Option<Renderer>,
}

struct Renderer {
    target: ID2D1HwndRenderTarget,
    heading_format: IDWriteTextFormat,
    instructions_format: IDWriteTextFormat,
    brush: windows::Win32::Graphics::Direct2D::ID2D1SolidColorBrush,
}

impl Renderer {
    unsafe fn new(hwnd: HWND, width: u32, height: u32) -> windows::core::Result<Self> {
        let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let target_properties = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            ..Default::default()
        };
        let hwnd_properties = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd: D2dHwnd(hwnd),
            pixelSize: D2D_SIZE_U { width, height },
            presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY,
        };
        let target = d2d.CreateHwndRenderTarget(&target_properties, &hwnd_properties)?;
        target.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

        let heading_format = dwrite.CreateTextFormat(
            w!("Segoe UI Variable Display"),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            18.0,
            w!("en-us"),
        )?;
        heading_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
        heading_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let instructions_format = dwrite.CreateTextFormat(
            w!("Segoe UI Variable Text"),
            None,
            DWRITE_FONT_WEIGHT_NORMAL,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            13.0,
            w!("en-us"),
        )?;
        instructions_format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
        instructions_format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        let brush = target.CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            },
            None,
        )?;
        Ok(Self {
            target,
            heading_format,
            instructions_format,
            brush,
        })
    }

    unsafe fn draw(&self, surface: &SurfaceText, width: f32, height: f32) {
        self.target.BeginDraw();
        self.target.Clear(Some(&D2D1_COLOR_F {
            r: 0.105,
            g: 0.115,
            b: 0.14,
            a: 0.38,
        }));
        let bounds = D2D_RECT_F {
            left: 0.5,
            top: 0.5,
            right: width - 0.5,
            bottom: height - 0.5,
        };
        let radius = surface.radius as f32;
        self.brush.SetColor(&D2D1_COLOR_F {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.22,
        });
        self.target.DrawRoundedRectangle(
            &D2D1_ROUNDED_RECT {
                rect: bounds,
                radiusX: radius,
                radiusY: radius,
            },
            &self.brush,
            1.0,
            None,
        );
        self.brush.SetColor(&D2D1_COLOR_F {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.96,
        });
        let heading = &surface.heading[..surface.heading.len().saturating_sub(1)];
        let instructions = &surface.instructions[..surface.instructions.len().saturating_sub(1)];
        self.target.DrawText(
            heading,
            &self.heading_format,
            &D2D_RECT_F {
                left: 28.0,
                top: 18.0,
                right: width - 24.0,
                bottom: 56.0,
            },
            &self.brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        self.brush.SetColor(&D2D1_COLOR_F {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.70,
        });
        self.target.DrawText(
            instructions,
            &self.instructions_format,
            &D2D_RECT_F {
                left: 28.0,
                top: 56.0,
                right: width - 24.0,
                bottom: 94.0,
            },
            &self.brush,
            D2D1_DRAW_TEXT_OPTIONS_NONE,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        let _ = self.target.EndDraw(None, None);
    }
}

fn main() {
    unsafe {
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
            fail("RegisterClassW failed");
        }

        let island = create_surface(
            instance,
            &class_name,
            ISLAND_WIDTH,
            ISLAND_HEIGHT,
            ISLAND_HEIGHT / 2,
            "DeskAct · Native island probe",
            "F1 opens separate Settings. Click elsewhere to test inactive Acrylic. Esc quits.",
            true,
        );
        ISLAND.store(island as isize, Ordering::Relaxed);

        let settings = create_surface(
            instance,
            &class_name,
            SETTINGS_WIDTH,
            SETTINGS_HEIGHT,
            24,
            "Settings · Independent native window",
            "This window has its own HWND and fixed size. Esc returns to the island.",
            false,
        );
        SETTINGS.store(settings as isize, Ordering::Relaxed);

        ShowWindow(island, SW_SHOW);
        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_surface(
    instance: HINSTANCE,
    class_name: &[u16],
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
    let text = Box::into_raw(Box::new(SurfaceText {
        heading: wide(heading),
        instructions: wide(instructions),
        radius,
        renderer: None,
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
        text.cast(),
    );
    if hwnd.is_null() {
        drop(Box::from_raw(text));
        fail("CreateWindowExW failed");
    }

    let acrylic_api_applied = apply_acrylic(hwnd);
    let data = &mut *text;
    data.instructions = wide(if acrylic_api_applied {
        "Native Acrylic API applied · F1 Settings · click away to test inactive"
    } else {
        "Native Acrylic API unavailable · F1 Settings · Esc quits"
    });
    apply_rounded_region(hwnd, radius);
    let mut client: RECT = zeroed();
    if GetClientRect(hwnd, &mut client) == 0 {
        fail("GetClientRect failed");
    }
    let data = &mut *text;
    data.renderer = Some(
        Renderer::new(hwnd, client.right as u32, client.bottom as u32)
            .unwrap_or_else(|_| fail("Direct2D renderer initialization failed")),
    );
    hwnd
}

unsafe fn apply_acrylic(hwnd: HWND) -> bool {
    // Use the older composition accent material, not DWMSBT_TRANSIENTWINDOW.
    // This is the path Wisp uses for Acrylic that must remain visible while inactive.
    const DWMWA_SYSTEMBACKDROP_TYPE: u32 = 38;
    const DWMSBT_NONE: u32 = 1;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWA_SYSTEMBACKDROP_TYPE,
        (&DWMSBT_NONE as *const u32).cast(),
        size_of::<u32>() as u32,
    );
    let margins = MARGINS {
        cxLeftWidth: 0,
        cxRightWidth: 0,
        cyTopHeight: 0,
        cyBottomHeight: 0,
    };
    let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);

    let user32 = wide("user32.dll");
    let module = GetModuleHandleW(user32.as_ptr());
    if module.is_null() {
        return false;
    }
    let proc = GetProcAddress(module, c"SetWindowCompositionAttribute".as_ptr().cast());
    let Some(proc) = proc else { return false };
    let set: SetWindowCompositionAttributeFn = std::mem::transmute(proc);
    let mut accent = AccentPolicy {
        state: 4, // ACCENT_ENABLE_ACRYLICBLURBEHIND
        flags: 2,
        tint: 0x80202028,
        animation_id: 0,
    };
    let mut data = WindowCompositionAttributeData {
        attribute: 19, // WCA_ACCENT_POLICY
        data: &mut accent,
        size: size_of::<AccentPolicy>(),
    };
    set(hwnd, &mut data) != 0
}

unsafe fn apply_rounded_region(hwnd: HWND, radius: i32) {
    let mut rect: RECT = zeroed();
    if GetWindowRect(hwnd, &mut rect) == 0 {
        return;
    }
    // SetWindowRgn coordinates are relative to the outer window bounds.
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
            let dc = BeginPaint(hwnd, &mut paint);
            let data = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SurfaceText;
            if !dc.is_null() && !data.is_null() {
                let mut client: RECT = zeroed();
                let _ = GetClientRect(hwnd, &mut client);
                if let Some(renderer) = (*data).renderer.as_ref() {
                    renderer.draw(&*data, client.right as f32, client.bottom as f32);
                }
            }
            EndPaint(hwnd, &paint);
            0
        }
        WM_KEYDOWN if wparam as u32 == VK_F1 as u32 && hwnd == handle(&ISLAND) => {
            let settings = handle(&SETTINGS);
            ShowWindow(settings, SW_SHOW);
            let _ = SetForegroundWindow(settings);
            0
        }
        WM_KEYDOWN if wparam as u32 == VK_ESCAPE as u32 && hwnd == handle(&SETTINGS) => {
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
            let text = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SurfaceText;
            if !text.is_null() {
                drop(Box::from_raw(text));
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            }
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn handle(value: &AtomicIsize) -> HWND {
    value.load(Ordering::Relaxed) as HWND
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn fail(message: &str) -> ! {
    eprintln!("native island probe: {message}");
    std::process::exit(1);
}
