use std::{cell::RefCell, io, ptr};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::*,
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};

use crate::{
    REGION_READY,
    frame::{Area, Frame},
    storage::wide,
    ui,
};

const CLASS: &str = "RustShot.Region";

thread_local! {
    static SELECTION: RefCell<Option<Selection>> = const { RefCell::new(None) };
}

struct Selection {
    parent: HWND,
    desktop: Area,
    frame: Frame,
    dim: Frame,
    anchor: Option<(i32, i32)>,
    cursor: (i32, i32),
    result: Option<Area>,
    finished: bool,
}

impl Selection {
    fn area(&self) -> Option<Area> {
        self.anchor.map(|a| Area::between(a, self.cursor))
    }

    fn point(&self) -> (i32, i32) {
        let mut position = POINT { x: 0, y: 0 };
        if unsafe { GetCursorPos(&mut position) } == 0 {
            return self.cursor;
        }
        (
            (position.x - self.desktop.x).clamp(0, self.desktop.w),
            (position.y - self.desktop.y).clamp(0, self.desktop.h),
        )
    }

    fn paint(&self, hwnd: HWND) {
        unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let canvas = Area {
                x: 0,
                y: 0,
                w: self.frame.w,
                h: self.frame.h,
            };
            self.dim.draw(dc, canvas);
            if let Some(a) = self.area().filter(|a| a.w > 0 && a.h > 0) {
                let old = SaveDC(dc);
                if old != 0 {
                    IntersectClipRect(dc, a.x, a.y, a.x + a.w, a.y + a.h);
                    self.frame.draw(dc, canvas);
                    RestoreDC(dc, old);
                }
                let t = 1;
                for edge in [
                    ui::rect(a.x - t, a.y - t, a.w + t * 2, t),
                    ui::rect(a.x - t, a.y + a.h, a.w + t * 2, t),
                    ui::rect(a.x - t, a.y, t, a.h),
                    ui::rect(a.x + a.w, a.y, t, a.h),
                ] {
                    ui::fill(dc, ui::ACCENT, edge);
                }
            }
            let primary_w = GetSystemMetrics(SM_CXSCREEN);
            let width = 485.min(primary_w.saturating_sub(24)).max(1);
            let x = -self.desktop.x + (primary_w - width) / 2;
            let y = -self.desktop.y + 24;
            let banner = ui::rect(x, y, width, 38);
            ui::fill(dc, ui::rgb(15, 34, 46), banner);
            let font = ui::font(16, 600);
            ui::write(
                dc,
                "Выделите область мышью    ·    Esc — отмена",
                banner,
                ui::ACCENT,
                font,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_END_ELLIPSIS,
            );
            DeleteObject(font);
            EndPaint(hwnd, &ps);
        }
    }
}

pub fn register(instance: HINSTANCE) -> io::Result<()> {
    let name = wide(CLASS);
    let class = WNDCLASSW {
        lpfnWndProc: Some(proc),
        hInstance: instance,
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_CROSS) },
        lpszClassName: name.as_ptr(),
        ..unsafe { std::mem::zeroed() }
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub fn begin(parent: HWND, instance: HINSTANCE, desktop: Area, frame: Frame) -> io::Result<()> {
    let dim = frame.dimmed();
    SELECTION.with_borrow_mut(|state| {
        *state = Some(Selection {
            parent,
            desktop,
            frame,
            dim,
            anchor: None,
            cursor: (0, 0),
            result: None,
            finished: false,
        });
    });
    let class = wide(CLASS);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            class.as_ptr(),
            wide("Выбор области RustShot").as_ptr(),
            WS_POPUP,
            desktop.x,
            desktop.y,
            desktop.w,
            desktop.h,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        )
    };
    if hwnd.is_null() {
        let error = io::Error::last_os_error();
        SELECTION.with_borrow_mut(|state| *state = None);
        return Err(error);
    }
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        SetFocus(hwnd);
    }
    Ok(())
}

pub fn take() -> Option<Frame> {
    SELECTION.with_borrow_mut(|state| {
        let selection = state.take()?;
        selection.frame.crop(selection.result?)
    })
}

fn finish(hwnd: HWND, save: bool) {
    let parent = SELECTION.with_borrow_mut(|state| {
        let selection = state.as_mut()?;
        if selection.finished {
            return None;
        }
        if save {
            selection.cursor = selection.point();
            selection.result = selection.area().filter(|a| a.w >= 1 && a.h >= 1);
        }
        selection.finished = true;
        Some(selection.parent)
    });
    if let Some(parent) = parent {
        unsafe {
            if GetCapture() == hwnd {
                ReleaseCapture();
            }
            DestroyWindow(hwnd);
            PostMessageW(parent, REGION_READY, 0, 0);
        }
    }
}

fn drag(hwnd: HWND) {
    let dirty = SELECTION.with_borrow_mut(|state| {
        let selection = state.as_mut()?;
        let before = selection.area()?;
        let point = selection.point();
        if selection.cursor == point {
            return None;
        }
        selection.cursor = point;
        let after = selection.area()?;
        let left = before.x.min(after.x) - 1;
        let top = before.y.min(after.y) - 1;
        let right = (before.x + before.w).max(after.x + after.w) + 1;
        let bottom = (before.y + before.h).max(after.y + after.h) + 1;
        Some(ui::rect(left, top, right - left, bottom - top))
    });
    if let Some(dirty) = dirty {
        unsafe { InvalidateRect(hwnd, &dirty, 0) };
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            SELECTION.with_borrow(|state| {
                if let Some(selection) = state.as_ref() {
                    selection.paint(hwnd);
                } else {
                    unsafe {
                        ValidateRect(hwnd, ptr::null());
                    }
                }
            });
        }
        WM_ERASEBKGND => return 1,
        WM_LBUTTONDOWN => {
            let begin = SELECTION.with_borrow_mut(|state| {
                if let Some(s) = state.as_mut().filter(|s| s.anchor.is_none() && !s.finished) {
                    s.cursor = s.point();
                    s.anchor = Some(s.cursor);
                    return true;
                }
                false
            });
            if begin {
                unsafe { SetCapture(hwnd) };
            }
        }
        WM_MOUSEMOVE => drag(hwnd),
        WM_LBUTTONUP => finish(hwnd, true),
        WM_KEYDOWN if wparam == VK_ESCAPE as usize => finish(hwnd, false),
        WM_RBUTTONDOWN | WM_CLOSE | WM_CANCELMODE | WM_DISPLAYCHANGE => finish(hwnd, false),
        WM_CAPTURECHANGED => {
            let dragging = SELECTION.with_borrow(|state| {
                state
                    .as_ref()
                    .is_some_and(|s| s.anchor.is_some() && !s.finished)
            });
            if dragging {
                finish(hwnd, false);
            }
        }
        WM_ACTIVATE if (wparam & 0xffff) == WA_INACTIVE as usize => {
            // Defer destruction so it cannot re-enter activation processing.
            unsafe { PostMessageW(hwnd, WM_CLOSE, 0, 0) };
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    0
}
