#![cfg_attr(not(test), windows_subsystem = "windows")]

mod app;
mod frame;
mod history;
mod hotkeys;
mod overlay;
mod settings;
mod storage;
mod ui;

const REGION_READY: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 1;
const SAVE_READY: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 2;
const TRAY_EVENT: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 3;
const EXIT_REQUEST: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 4;
const HISTORY_READY: u32 = windows_sys::Win32::UI::WindowsAndMessaging::WM_APP + 5;

fn main() {
    if let Err(error) = app::run() {
        let message = storage::wide(format!("Не удалось запустить RustShot.\n\n{error}"));
        let title = storage::wide("RustShot");
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::*;
            MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                MB_ICONERROR,
            );
        }
    }
}
