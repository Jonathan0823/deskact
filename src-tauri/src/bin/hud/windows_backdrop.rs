//! A rounded Windows Composition host-backdrop visual behind the WebView.
//!
//! Unlike DWMSBT_TRANSIENTWINDOW, this material belongs to the app's visual tree
//! and is clipped geometrically. It uses Windows.UI.Composition built into
//! Windows 11; it does not require the Windows App SDK runtime.

use std::{cell::RefCell, mem::size_of};

use windows::{
    Foundation::TimeSpan,
    System::{DispatcherQueue, DispatcherQueueController},
    UI::Composition::{
        CompositionRoundedRectangleGeometry, Compositor, ContainerVisual,
        Desktop::DesktopWindowTarget, ScalarKeyFrameAnimation, SpriteVisual,
    },
    Win32::{
        Foundation::HWND,
        System::WinRT::{
            Composition::ICompositorDesktopInterop, CreateDispatcherQueueController, DQTAT_COM_STA,
            DQTYPE_THREAD_CURRENT, DispatcherQueueOptions, RO_INIT_SINGLETHREADED, RoInitialize,
            RoUninitialize,
        },
    },
    core::{HSTRING, Interface, Result},
};
use windows_numerics::Vector2;
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Dwm::{
        DWMSBT_NONE, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_HOSTBACKDROPBRUSH, DwmSetWindowAttribute,
    },
    UI::{HiDpi::GetDpiForWindow, WindowsAndMessaging::GetWindowRect},
};

type NativeHwnd = windows_sys::Win32::Foundation::HWND;

thread_local! {
    // Composition objects are apartment-bound and stay on Tauri's UI thread.
    static STATE: RefCell<BackdropState> = RefCell::new(BackdropState::default());
}

#[derive(Default)]
struct BackdropState {
    runtime: Option<CompositionRuntime>,
    surface: Option<BackdropSurface>,
    enabled: bool,
}

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

struct CompositionRuntime {
    compositor: Compositor,
    _queue: Option<DispatcherQueueController>,
    _apartment: Apartment,
}

impl CompositionRuntime {
    unsafe fn new() -> Result<Self> {
        unsafe { RoInitialize(RO_INIT_SINGLETHREADED)? };
        // Keep a matching RoUninitialize even if dispatcher/compositor creation fails.
        let apartment = Apartment;
        let queue = if DispatcherQueue::GetForCurrentThread().is_ok() {
            None
        } else {
            Some(unsafe {
                CreateDispatcherQueueController(DispatcherQueueOptions {
                    dwSize: size_of::<DispatcherQueueOptions>() as u32,
                    threadType: DQTYPE_THREAD_CURRENT,
                    apartmentType: DQTAT_COM_STA,
                })?
            })
        };
        Ok(Self {
            compositor: Compositor::new()?,
            _queue: queue,
            _apartment: apartment,
        })
    }
}

struct BackdropSurface {
    hwnd: NativeHwnd,
    target: DesktopWindowTarget,
    root: ContainerVisual,
    sprite: SpriteVisual,
    geometry: CompositionRoundedRectangleGeometry,
}

impl BackdropSurface {
    unsafe fn new(runtime: &CompositionRuntime, hwnd: NativeHwnd) -> Result<Self> {
        let compositor = &runtime.compositor;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        // Insert below child HWNDs so WebView2 remains the interactive top layer.
        let target = unsafe { interop.CreateDesktopWindowTarget(HWND(hwnd), false)? };
        let root = compositor.CreateContainerVisual()?;
        let sprite = compositor.CreateSpriteVisual()?;
        let geometry = compositor.CreateRoundedRectangleGeometry()?;
        root.SetClip(&compositor.CreateGeometricClipWithGeometry(&geometry)?)?;
        sprite.SetBrush(&compositor.CreateHostBackdropBrush()?)?;
        root.Children()?.InsertAtTop(&sprite)?;
        target.SetRoot(&root)?;
        Ok(Self {
            hwnd,
            target,
            root,
            sprite,
            geometry,
        })
    }

    fn prepare_for_show(&self) -> Result<()> {
        let property = HSTRING::from("Opacity");
        self.root.StopAnimation(&property)?;
        self.root.SetOpacity(0.0)?;
        self.root.SetIsVisible(true)
    }

    fn animate_visibility(&self, visible: bool, duration_ms: u32) -> Result<()> {
        let property = HSTRING::from("Opacity");
        let current = self.root.Opacity()?;
        self.root.StopAnimation(&property)?;
        self.root.SetOpacity(current)?;
        self.root.SetIsVisible(true)?;
        if duration_ms == 0 {
            return self.root.SetOpacity(if visible { 1.0 } else { 0.0 });
        }
        let compositor = self.root.Compositor()?;
        let easing = compositor.CreateCubicBezierEasingFunction(
            Vector2 { X: 0.22, Y: 1.0 },
            Vector2 { X: 0.36, Y: 1.0 },
        )?;
        let animation: ScalarKeyFrameAnimation = compositor.CreateScalarKeyFrameAnimation()?;
        animation.SetDuration(TimeSpan {
            Duration: i64::from(duration_ms) * 10_000,
        })?;
        animation.InsertKeyFrameWithEasingFunction(
            1.0,
            if visible { 1.0 } else { 0.0 },
            &easing,
        )?;
        self.root.StartAnimation(&property, &animation)
    }

    fn resize(&self, width: i32, height: i32, radius: usize, dpi: u32) -> Result<()> {
        // Composition visual coordinates are DIPs; HWND dimensions and corner radius are pixels.
        let scale = (dpi.max(96) as f32) / 96.0;
        let size = Vector2 {
            X: width.max(0) as f32 / scale,
            Y: height.max(0) as f32 / scale,
        };
        let radius = (radius as f32 / scale).min(size.X.min(size.Y) / 2.0);
        self.root.SetSize(size)?;
        self.sprite.SetSize(size)?;
        self.geometry.SetSize(size)?;
        self.geometry.SetCornerRadius(Vector2 {
            X: radius,
            Y: radius,
        })
    }
}

impl Drop for BackdropSurface {
    fn drop(&mut self) {
        let _ = self.target.Close();
    }
}

/// Enable or disable the clipped host-backdrop visual on the HWND's UI thread.
///
/// # Safety
/// `hwnd` must be a live island window owned by the calling thread.
pub unsafe fn set_material(
    hwnd: NativeHwnd,
    enabled: bool,
    window_visible: bool,
    radius: usize,
) -> std::result::Result<(), String> {
    // Never leave DWM's rectangular system backdrop behind the composition visual.
    let _ = unsafe { set_attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, DWMSBT_NONE) };
    if !enabled {
        STATE.with_borrow_mut(|state| {
            state.enabled = false;
            if let Some(surface) = &state.surface {
                let _ = surface.root.SetIsVisible(false);
            }
        });
        let _ = unsafe { set_attribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, 0) };
        return Ok(());
    }
    unsafe { set_attribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, 1)? };
    let result = STATE.with_borrow_mut(|state| {
        let result = (|| -> Result<()> {
            if state.runtime.is_none() {
                state.runtime = Some(unsafe { CompositionRuntime::new()? });
            }
            if state.surface.is_none() {
                state.surface =
                    Some(unsafe { BackdropSurface::new(state.runtime.as_ref().unwrap(), hwnd)? });
            }
            let surface = state.surface.as_ref().unwrap();
            if surface.hwnd != hwnd {
                return Err(windows::core::Error::new(
                    windows::core::HRESULT(0x8000_4005u32 as i32),
                    "Composition backdrop already belongs to another window",
                ));
            }
            let mut rect: RECT = unsafe { std::mem::zeroed() };
            if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
                return Err(windows::core::Error::from_thread());
            }
            surface.resize(
                rect.right - rect.left,
                rect.bottom - rect.top,
                radius,
                unsafe { GetDpiForWindow(hwnd) },
            )?;
            surface
                .root
                .SetOpacity(if window_visible { 1.0 } else { 0.0 })?;
            surface.root.SetIsVisible(true)?;
            state.enabled = true;
            Ok(())
        })();
        result.map_err(|error| format!("Could not enable the clipped Windows glass layer: {error}"))
    });
    if result.is_err() {
        STATE.with_borrow_mut(|state| {
            state.enabled = false;
            if let Some(surface) = &state.surface {
                let _ = surface.root.SetIsVisible(false);
            }
        });
        let _ = unsafe { set_attribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, 0) };
    }
    result
}

unsafe fn set_attribute(
    hwnd: NativeHwnd,
    attribute: i32,
    value: i32,
) -> std::result::Result<(), String> {
    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            attribute as u32,
            (&value as *const i32).cast(),
            size_of::<i32>() as u32,
        )
    };
    if result < 0 {
        Err(format!(
            "Windows backdrop attribute {attribute} is unavailable (HRESULT 0x{:08X})",
            result as u32
        ))
    } else {
        Ok(())
    }
}

/// Reset the native backdrop before the previously hidden HWND is shown.
pub fn prepare_for_show(hwnd: NativeHwnd) {
    STATE.with(|cell| {
        if let Ok(state) = cell.try_borrow()
            && state.enabled
            && let Some(surface) = &state.surface
            && surface.hwnd == hwnd
        {
            let _ = surface.prepare_for_show();
        }
    });
}

/// Animate the native backdrop in sync with the WebView's show/hide transition.
pub fn animate_visibility(
    hwnd: NativeHwnd,
    visible: bool,
    duration_ms: u32,
) -> std::result::Result<(), String> {
    STATE.with_borrow(|state| {
        if !state.enabled {
            return Ok(());
        }
        let Some(surface) = &state.surface else {
            return Ok(());
        };
        if surface.hwnd != hwnd {
            return Err("Composition backdrop belongs to another window".to_owned());
        }
        surface
            .animate_visibility(visible, duration_ms)
            .map_err(|error| format!("Could not animate the native glass layer: {error}"))
    })
}

/// Update the clipped visual before the HWND commits a native resize.
pub fn resize(hwnd: NativeHwnd, width: i32, height: i32, radius: usize) {
    STATE.with(|cell| {
        if let Ok(state) = cell.try_borrow()
            && state.enabled
            && let Some(surface) = &state.surface
            && surface.hwnd == hwnd
        {
            let dpi = unsafe { GetDpiForWindow(hwnd) };
            let _ = surface.resize(width, height, radius, dpi);
        }
    });
}

/// Release the composition target as the island HWND is destroyed.
pub fn detach(hwnd: NativeHwnd) {
    STATE.with(|cell| {
        if let Ok(mut state) = cell.try_borrow_mut()
            && state
                .surface
                .as_ref()
                .is_some_and(|surface| surface.hwnd == hwnd)
        {
            state.surface = None;
        }
    });
}
