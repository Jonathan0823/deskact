//! Windows.UI.Composition host-backdrop surface clipped to a rounded rectangle.

use windows::{
    UI::Composition::{
        CompositionGeometricClip, CompositionRoundedRectangleGeometry, Compositor, ContainerVisual,
        SpriteVisual,
    },
    Win32::System::WinRT::{
        CreateDispatcherQueueController, DQTAT_COM_STA, DQTYPE_THREAD_CURRENT,
        DispatcherQueueOptions, RO_INIT_SINGLETHREADED, RoInitialize,
    },
    core::{Interface, Result},
};
use windows_numerics::Vector2;

/// Runtime objects that must remain alive on the UI thread for Composition.
pub struct CompositionRuntime {
    pub compositor: Compositor,
    _dispatcher_queue: windows::System::DispatcherQueueController,
}

impl CompositionRuntime {
    /// Initialize WinRT and a current-thread dispatcher queue before creating a Compositor.
    pub unsafe fn new() -> Result<Self> {
        RoInitialize(RO_INIT_SINGLETHREADED)?;
        let options = DispatcherQueueOptions {
            dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_CURRENT,
            apartmentType: DQTAT_COM_STA,
        };
        let dispatcher_queue = CreateDispatcherQueueController(options)?;
        let compositor = Compositor::new()?;
        Ok(Self {
            compositor,
            _dispatcher_queue: dispatcher_queue,
        })
    }
}

/// Owns the Composition objects attached to one HWND.
pub struct CompositionBackdrop {
    _target: windows::UI::Composition::Desktop::DesktopWindowTarget,
    _root: ContainerVisual,
    _visual: SpriteVisual,
    _clip: CompositionGeometricClip,
    _geometry: CompositionRoundedRectangleGeometry,
}

impl CompositionBackdrop {
    /// Attach a host backdrop brush behind the HWND contents and clip it to a pill/rounded panel.
    pub unsafe fn attach(
        runtime: &CompositionRuntime,
        hwnd: windows::Win32::Foundation::HWND,
        width: f32,
        height: f32,
        corner_radius: f32,
    ) -> Result<Self> {
        let interop: windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop =
            runtime.compositor.cast()?;
        let target = interop.CreateDesktopWindowTarget(hwnd, false)?;
        let root = runtime.compositor.CreateContainerVisual()?;

        let geometry = runtime.compositor.CreateRoundedRectangleGeometry()?;
        geometry.SetSize(Vector2 {
            X: width,
            Y: height,
        })?;
        geometry.SetCornerRadius(Vector2 {
            X: corner_radius,
            Y: corner_radius,
        })?;
        let clip = runtime
            .compositor
            .CreateGeometricClipWithGeometry(&geometry)?;

        let visual = runtime.compositor.CreateSpriteVisual()?;
        visual.SetSize(Vector2 {
            X: width,
            Y: height,
        })?;
        visual.SetClip(&clip)?;
        visual.SetBrush(&runtime.compositor.CreateHostBackdropBrush()?)?;
        root.Children()?.InsertAtTop(&visual)?;
        target.SetRoot(&root)?;

        Ok(Self {
            _target: target,
            _root: root,
            _visual: visual,
            _clip: clip,
            _geometry: geometry,
        })
    }
}
