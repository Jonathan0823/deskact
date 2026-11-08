#include <windows.h>
#include <dwmapi.h>
#include <dispatcherqueue.h>
#include <MddBootstrap.h>
#include <winrt/Windows.Foundation.h>
#include <winrt/Windows.System.h>
#include <winrt/Windows.UI.Composition.h>
#include <winrt/Windows.UI.Composition.Desktop.h>
#include <winrt/Microsoft.UI.h>
#include <winrt/Microsoft.UI.Composition.SystemBackdrops.h>
#include <Windows.UI.Composition.Interop.h>

#include <cstdint>
#include <memory>
#include <new>

namespace {
using winrt::Microsoft::UI::Composition::SystemBackdrops::DesktopAcrylicController;
using winrt::Windows::UI::Composition::Compositor;
using winrt::Windows::UI::Composition::CompositionTarget;
using winrt::Windows::UI::Composition::ContainerVisual;
using winrt::Windows::UI::Composition::Desktop::DesktopWindowTarget;
using winrt::Windows::System::DispatcherQueueController;

struct BackdropState {
  DesktopWindowTarget target{nullptr};
  ContainerVisual root{nullptr};
  DesktopAcrylicController controller{nullptr};
};

bool g_bootstrap_initialized = false;
bool g_apartment_initialized = false;
DispatcherQueueController g_dispatcher_queue{nullptr};
Compositor g_compositor{nullptr};

HRESULT initialize_composition() noexcept {
  try {
    if (!g_apartment_initialized) {
      winrt::init_apartment(winrt::apartment_type::single_threaded);
      g_apartment_initialized = true;
    }

    if (g_dispatcher_queue == nullptr &&
        winrt::Windows::System::DispatcherQueue::GetForCurrentThread() == nullptr) {
      DispatcherQueueOptions options{
          sizeof(DispatcherQueueOptions), DQTYPE_THREAD_CURRENT, DQTAT_COM_STA};
      ABI::Windows::System::IDispatcherQueueController* queue = nullptr;
      winrt::check_hresult(CreateDispatcherQueueController(
          options, &queue));
      g_dispatcher_queue = {queue, winrt::take_ownership_from_abi};
    }

    if (g_compositor == nullptr) {
      g_compositor = Compositor();
    }
    return S_OK;
  } catch (const winrt::hresult_error& error) {
    return error.code();
  } catch (...) {
    return E_FAIL;
  }
}
}  // namespace

extern "C" __declspec(dllexport) HRESULT __cdecl deskact_bootstrap_initialize() noexcept {
  if (g_bootstrap_initialized) {
    return S_FALSE;
  }

  // Windows App SDK 2.6.5 (major/minor encoded as 0x00020006).
  const UINT32 major_minor_version = 0x00020006;
  const PACKAGE_VERSION minimum_version{};
  const HRESULT result = MddBootstrapInitialize(
      major_minor_version, L"", minimum_version);
  if (SUCCEEDED(result)) {
    g_bootstrap_initialized = true;
  }
  return result;
}

extern "C" __declspec(dllexport) HRESULT __cdecl deskact_acrylic_attach(
    HWND hwnd, void** output_state) noexcept {
  if (hwnd == nullptr || output_state == nullptr) {
    return E_POINTER;
  }
  *output_state = nullptr;
  if (!g_bootstrap_initialized) {
    return E_UNEXPECTED;
  }

  const HRESULT initialized = initialize_composition();
  if (FAILED(initialized)) {
    return initialized;
  }

  BOOL use_host_backdrop = TRUE;
  const HRESULT backdrop_attribute = DwmSetWindowAttribute(
      hwnd, static_cast<DWMWINDOWATTRIBUTE>(17), &use_host_backdrop,
      sizeof(use_host_backdrop));
  if (FAILED(backdrop_attribute)) {
    return backdrop_attribute;
  }

  try {
    auto interop = g_compositor.as<ABI::Windows::UI::Composition::Desktop::ICompositorDesktopInterop>();
    DesktopWindowTarget target{nullptr};
    winrt::check_hresult(interop->CreateDesktopWindowTarget(
        hwnd, true,
        reinterpret_cast<ABI::Windows::UI::Composition::Desktop::IDesktopWindowTarget**>(
            winrt::put_abi(target))));

    auto root = g_compositor.CreateContainerVisual();
    target.Root(root);

    auto controller = DesktopAcrylicController();
    const auto window_id = winrt::Microsoft::UI::WindowId{
        static_cast<std::uint64_t>(reinterpret_cast<std::uintptr_t>(hwnd))};
    const auto composition_target = target.as<CompositionTarget>();
    if (!controller.SetTarget(window_id, composition_target)) {
      controller.Close();
      return HRESULT_FROM_WIN32(ERROR_NOT_SUPPORTED);
    }

    auto state = std::make_unique<BackdropState>();
    state->target = std::move(target);
    state->root = std::move(root);
    state->controller = std::move(controller);
    *output_state = state.release();
    return S_OK;
  } catch (const winrt::hresult_error& error) {
    return error.code();
  } catch (...) {
    return E_FAIL;
  }
}

extern "C" __declspec(dllexport) void __cdecl deskact_acrylic_detach(
    void* opaque_state) noexcept {
  auto state = std::unique_ptr<BackdropState>(
      static_cast<BackdropState*>(opaque_state));
  if (state == nullptr) {
    return;
  }
  try {
    if (state->controller != nullptr) {
      state->controller.Close();
      state->controller = nullptr;
    }
    if (state->target != nullptr) {
      state->target.Root(nullptr);
    }
  } catch (...) {
    // Teardown must not throw through the C ABI.
  }
}

extern "C" __declspec(dllexport) void __cdecl deskact_composition_shutdown() noexcept {
  try {
    g_compositor = nullptr;
    if (g_dispatcher_queue != nullptr) {
      g_dispatcher_queue.ShutdownQueueAsync();
      g_dispatcher_queue = nullptr;
    }
    if (g_apartment_initialized) {
      winrt::uninit_apartment();
      g_apartment_initialized = false;
    }
  } catch (...) {
  }
}

extern "C" __declspec(dllexport) void __cdecl deskact_bootstrap_shutdown() noexcept {
  if (g_bootstrap_initialized) {
    MddBootstrapShutdown();
    g_bootstrap_initialized = false;
  }
}
