use std::io;

use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::{Dwm::*, Gdi::*},
    UI::WindowsAndMessaging::*,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    pub fn between(a: (i32, i32), b: (i32, i32)) -> Self {
        Self {
            x: a.0.min(b.0),
            y: a.1.min(b.1),
            w: a.0.max(b.0) - a.0.min(b.0),
            h: a.1.max(b.1) - a.1.min(b.1),
        }
    }

    pub fn intersect(self, other: Self) -> Option<Self> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let w = (self.x + self.w).min(other.x + other.w) - x;
        let h = (self.y + self.h).min(other.y + other.h) - y;
        (w > 0 && h > 0).then_some(Self { x, y, w, h })
    }
}

pub struct Frame {
    pub w: i32,
    pub h: i32,
    // Top-down BGRX, one four-byte pixel at a time.
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn byte_len(w: i32, h: i32) -> Option<usize> {
        if w <= 0 || h <= 0 {
            return None;
        }
        (w as usize)
            .checked_mul(h as usize)?
            .checked_mul(4)
            .filter(|&bytes| bytes <= 256 * 1024 * 1024)
    }

    pub fn crop(&self, area: Area) -> Option<Self> {
        let bounds = Area {
            x: 0,
            y: 0,
            w: self.w,
            h: self.h,
        };
        let area = bounds.intersect(area)?;
        let stride = self.w as usize * 4;
        let row = area.w as usize * 4;
        let mut pixels = Vec::with_capacity(Self::byte_len(area.w, area.h)?);
        for y in area.y..area.y + area.h {
            let start = y as usize * stride + area.x as usize * 4;
            pixels.extend_from_slice(&self.pixels[start..start + row]);
        }
        Some(Self {
            w: area.w,
            h: area.h,
            pixels,
        })
    }

    pub fn dimmed(&self) -> Self {
        let pixels = self.pixels.iter().map(|&c| c / 2).collect();
        Self {
            w: self.w,
            h: self.h,
            pixels,
        }
    }

    pub fn fit(&self, bounds: Area) -> Area {
        let ratio = (bounds.w as f64 / self.w as f64).min(bounds.h as f64 / self.h as f64);
        let w = ((self.w as f64 * ratio) as i32).max(1);
        let h = ((self.h as f64 * ratio) as i32).max(1);
        Area {
            x: bounds.x + (bounds.w - w) / 2,
            y: bounds.y + (bounds.h - h) / 2,
            w,
            h,
        }
    }

    pub fn draw(&self, dc: HDC, dst: Area) {
        let info = bitmap_info(self.w, self.h);
        unsafe {
            StretchDIBits(
                dc,
                dst.x,
                dst.y,
                dst.w,
                dst.h,
                0,
                0,
                self.w,
                self.h,
                self.pixels.as_ptr().cast(),
                &info,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }
    }
}

fn bitmap_info(w: i32, h: i32) -> BITMAPINFO {
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = w;
    info.bmiHeader.biHeight = -h;
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB;
    info
}

pub fn desktop() -> Area {
    unsafe {
        Area {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            w: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            h: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

pub fn active_window(app: HWND) -> Option<Area> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null()
            || hwnd == app
            || hwnd == GetDesktopWindow()
            || hwnd == GetShellWindow()
            || IsIconic(hwnd) != 0
            || IsWindowVisible(hwnd) == 0
        {
            return None;
        }
        let mut rect: RECT = std::mem::zeroed();
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS as u32,
            (&mut rect as *mut RECT).cast(),
            std::mem::size_of::<RECT>() as u32,
        ) < 0
            && GetWindowRect(hwnd, &mut rect) == 0
        {
            return None;
        }
        desktop().intersect(Area::between(
            (rect.left, rect.top),
            (rect.right, rect.bottom),
        ))
    }
}

pub fn capture(area: Area) -> io::Result<Frame> {
    let bytes = Frame::byte_len(area.w, area.h)
        .ok_or_else(|| io::Error::other("Недопустимый размер снимка"))?;
    let info = bitmap_info(area.w, area.h);
    unsafe {
        let screen = GetDC(std::ptr::null_mut());
        if screen.is_null() {
            return Err(io::Error::last_os_error());
        }
        let memory = CreateCompatibleDC(screen);
        if memory.is_null() {
            let error = io::Error::last_os_error();
            ReleaseDC(std::ptr::null_mut(), screen);
            return Err(error);
        }
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(
            screen,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        let result = if !bitmap.is_null() && !bits.is_null() {
            // Areas between monitors may not be written by the screen DC.
            std::ptr::write_bytes(bits.cast::<u8>(), 0, bytes);
            let previous = SelectObject(memory, bitmap);
            if !previous.is_null()
                && BitBlt(
                    memory,
                    0,
                    0,
                    area.w,
                    area.h,
                    screen,
                    area.x,
                    area.y,
                    SRCCOPY | CAPTUREBLT,
                ) != 0
            {
                GdiFlush();
                let pixels = std::slice::from_raw_parts(bits.cast::<u8>(), bytes).to_vec();
                SelectObject(memory, previous);
                Ok(Frame {
                    w: area.w,
                    h: area.h,
                    pixels,
                })
            } else {
                let error = io::Error::last_os_error();
                if !previous.is_null() {
                    SelectObject(memory, previous);
                }
                Err(error)
            }
        } else {
            Err(io::Error::last_os_error())
        };
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        DeleteDC(memory);
        ReleaseDC(std::ptr::null_mut(), screen);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersects_negative_desktop_coordinates() {
        let desktop = Area {
            x: -1920,
            y: -100,
            w: 4480,
            h: 1540,
        };
        let window = Area {
            x: -2000,
            y: 1200,
            w: 420,
            h: 500,
        };
        assert_eq!(
            desktop.intersect(window),
            Some(Area {
                x: -1920,
                y: 1200,
                w: 340,
                h: 240
            })
        );
        assert!(
            desktop
                .intersect(Area {
                    x: 2560,
                    y: 0,
                    w: 10,
                    h: 10
                })
                .is_none()
        );
    }

    #[test]
    fn reverse_drag_crops_bottom_right_pixels() {
        let frame = Frame {
            w: 3,
            h: 2,
            pixels: (0..24).collect(),
        };
        let crop = frame.crop(Area::between((3, 2), (1, 1))).unwrap();
        assert_eq!((crop.w, crop.h, crop.pixels), (2, 1, (16..24).collect()));
    }

    #[test]
    fn rejects_invalid_or_oversized_frames() {
        assert!(Frame::byte_len(1, 0).is_none());
        assert!(Frame::byte_len(i32::MAX, i32::MAX).is_none());
    }
}
