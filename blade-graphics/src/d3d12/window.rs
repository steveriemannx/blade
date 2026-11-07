//! A hidden top-level window, used by the swapchain example and tests.
//!
//! The device layer itself takes an `HWND` from whatever windowing library the
//! application uses (`blade` uses `winit`); this helper only exists so the
//! swapchain path can be exercised headlessly over SSH.

use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SW_SHOWNORMAL,
    ShowWindow, WINDOW_EX_STYLE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};
use windows::core::PCWSTR;

use super::Error;

static WINDOW_CLASS_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A hidden window with a registered class; destroyed on drop.
pub struct HiddenWindow {
    hwnd: HWND,
}

impl HiddenWindow {
    pub fn new(title: &str, width: u32, height: u32) -> Result<Self, Error> {
        let module = unsafe { GetModuleHandleW(None)? };
        let instance = HINSTANCE(module.0);
        let class_name = format!(
            "blade-d3d12-window-{}",
            WINDOW_CLASS_COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let class_name_wide = to_wide(&class_name);
        let title_wide = to_wide(title);

        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(class_name_wide.as_ptr()),
            ..Default::default()
        };
        // The class stays registered for the process; re-registering it after a
        // previous window of this process was destroyed is not an error here.
        unsafe { RegisterClassW(&class) };

        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name_wide.as_ptr()),
                PCWSTR(title_wide.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                width as i32,
                height as i32,
                None,
                None,
                Some(instance),
                None,
            )?
        };
        Ok(Self { hwnd })
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Shows the window. DXGI only lets a swapchain resize once the presentation
    /// engine has consumed its frames, which does not happen while the window is
    /// hidden, so swapchain tests/examples show the window after creating the
    /// swapchain.
    pub fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNORMAL);
        }
    }
}

impl Drop for HiddenWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
