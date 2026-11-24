use std::{io, path::Path, ptr};

use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::*, Input::KeyboardAndMouse::EnableWindow, WindowsAndMessaging::*},
};

use crate::{
    frame::{Area, Frame},
    hotkeys::{Hotkeys, Mode},
    settings::CloseAction,
    storage::wide,
};

pub const WIDTH: i32 = 900;
pub const HEIGHT: i32 = 610;
pub const REGION: usize = 101;
pub const DESKTOP: usize = 102;
pub const WINDOW: usize = 103;
pub const FOLDER: usize = 104;
pub const COPY: usize = 105;
pub const OPEN: usize = 106;
pub const SETTINGS: usize = 107;
pub const CLOSE_TO_TRAY: usize = 108;
pub const CLOSE_TO_EXIT: usize = 109;
pub const CHOOSE_FOLDER: usize = 110;
pub const RESET_FOLDER: usize = 111;
pub const HISTORY: usize = 120;
pub const SLOTS: usize = 6;

const BG: u32 = rgb(14, 23, 37);
const HEADER: u32 = rgb(18, 30, 47);
const PANEL: u32 = rgb(20, 33, 51);
const EDGE: u32 = rgb(39, 56, 77);
const SURFACE: u32 = rgb(10, 18, 30);
const TEXT: u32 = rgb(238, 245, 253);
const MUTED: u32 = rgb(166, 184, 205);
pub const ACCENT: u32 = rgb(111, 235, 205);
const DARK_TEAL: u32 = rgb(25, 63, 66);
const WARN: u32 = rgb(255, 198, 135);

pub const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}

pub fn rect(x: i32, y: i32, w: i32, h: i32) -> RECT {
    RECT {
        left: x,
        top: y,
        right: x + w,
        bottom: y + h,
    }
}

pub fn fill(dc: HDC, color: u32, rect: RECT) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, &rect, brush);
        DeleteObject(brush);
    }
}

fn border(dc: HDC, color: u32, rect: RECT) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FrameRect(dc, &rect, brush);
        DeleteObject(brush);
    }
}

pub fn write(dc: HDC, label: &str, mut bounds: RECT, color: u32, font: HFONT, flags: u32) {
    let label = wide(label);
    unsafe {
        let old = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color);
        DrawTextW(dc, label.as_ptr(), -1, &mut bounds, DT_NOPREFIX | flags);
        if !old.is_null() {
            SelectObject(dc, old);
        }
    }
}

pub fn font(size: i32, weight: i32) -> HFONT {
    unsafe {
        CreateFontW(
            -size.max(1),
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            DEFAULT_PITCH as u32,
            wide("Segoe UI").as_ptr(),
        )
    }
}

const LINE: u32 = DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS;

#[derive(Clone, Copy)]
struct Scale(f64);

impl Scale {
    fn px(self, n: i32) -> i32 {
        (n as f64 * self.0).round() as i32
    }

    fn rect(self, x: i32, y: i32, w: i32, h: i32) -> RECT {
        RECT {
            left: self.px(x),
            top: self.px(y),
            right: self.px(x + w),
            bottom: self.px(y + h),
        }
    }
}

struct Fonts {
    small: HFONT,
    normal: HFONT,
    medium: HFONT,
    brand: HFONT,
}

impl Fonts {
    fn new(s: Scale) -> Self {
        Self {
            small: font(s.px(13), 400),
            normal: font(s.px(15), 400),
            medium: font(s.px(17), 600),
            brand: font(s.px(34), 700),
        }
    }
}

impl Drop for Fonts {
    fn drop(&mut self) {
        for h in [self.small, self.normal, self.medium, self.brand] {
            unsafe { DeleteObject(h) };
        }
    }
}

pub struct Ui {
    controls: Vec<(usize, HWND)>,
    scale: Scale,
    fonts: Fonts,
}

pub struct Dashboard<'a> {
    pub preview: Option<&'a Frame>,
    pub preview_loading: bool,
    pub history_loading: bool,
    pub history_len: usize,
    pub settings_page: bool,
    pub save_dir: &'a Path,
    pub status: &'a str,
    pub warn: bool,
    pub hotkeys: usize,
}

impl Ui {
    pub fn new(hwnd: HWND) -> io::Result<Self> {
        let mut controls = Vec::new();
        let all = [
            (REGION, "Область"),
            (DESKTOP, "Весь экран"),
            (WINDOW, "Активное окно"),
            (SETTINGS, "Настройки"),
            (FOLDER, "Открыть папку"),
            (COPY, "Копировать"),
            (OPEN, "Открыть PNG"),
            (CLOSE_TO_TRAY, "Свернуть в трей"),
            (CLOSE_TO_EXIT, "Закрыть программу"),
            (CHOOSE_FOLDER, "Выбрать папку…"),
            (RESET_FOLDER, "По умолчанию"),
        ]
        .into_iter()
        .chain((0..SLOTS).map(|i| (HISTORY + i, "")));
        let class = wide("BUTTON");
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        for (id, label) in all {
            let label = wide(label);
            let style = WS_CHILD
                | WS_TABSTOP
                | BS_OWNERDRAW as u32
                | if id < CLOSE_TO_TRAY { WS_VISIBLE } else { 0 };
            let button = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    label.as_ptr(),
                    style,
                    0,
                    0,
                    1,
                    1,
                    hwnd,
                    id as _,
                    instance,
                    ptr::null(),
                )
            };
            if button.is_null() {
                return Err(io::Error::last_os_error());
            }
            controls.push((id, button));
        }
        let mut ui = Self {
            controls,
            scale: Scale(1.0),
            fonts: Fonts::new(Scale(1.0)),
        };
        ui.layout(hwnd);
        Ok(ui)
    }

    pub fn layout(&mut self, hwnd: HWND) {
        let mut client = rect(0, 0, 0, 0);
        unsafe { GetClientRect(hwnd, &mut client) };
        if client.right <= 0 || client.bottom <= 0 {
            return;
        }
        let scale =
            Scale((client.right as f64 / WIDTH as f64).min(client.bottom as f64 / HEIGHT as f64));
        if (scale.0 - self.scale.0).abs() > 0.005 {
            self.fonts = Fonts::new(scale);
        }
        self.scale = scale;
        for &(id, button) in &self.controls {
            let bounds = match id {
                REGION => scale.rect(32, 153, 268, 68),
                DESKTOP => scale.rect(316, 153, 268, 68),
                WINDOW => scale.rect(600, 153, 268, 68),
                SETTINGS => scale.rect(562, 42, 134, 37),
                FOLDER => scale.rect(708, 42, 160, 37),
                COPY => scale.rect(49, 503, 125, 32),
                OPEN => scale.rect(183, 503, 125, 32),
                CLOSE_TO_TRAY => scale.rect(56, 249, 385, 80),
                CLOSE_TO_EXIT => scale.rect(459, 249, 385, 80),
                CHOOSE_FOLDER => scale.rect(56, 501, 172, 34),
                RESET_FOLDER => scale.rect(240, 501, 148, 34),
                _ => scale.rect(617, 302 + (id - HISTORY) as i32 * 38, 236, 32),
            };
            unsafe {
                SetWindowPos(
                    button,
                    ptr::null_mut(),
                    bounds.left,
                    bounds.top,
                    bounds.right - bounds.left,
                    bounds.bottom - bounds.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                InvalidateRect(button, ptr::null(), 0);
            }
        }
        unsafe { InvalidateRect(hwnd, ptr::null(), 0) };
    }

    pub fn update(
        &self,
        idle: bool,
        preview: bool,
        can_open: bool,
        history: &[String],
        settings_page: bool,
        custom_folder: bool,
    ) {
        for &(id, control) in &self.controls {
            unsafe {
                let visibility = if settings_page { SW_HIDE } else { SW_SHOWNA };
                match id {
                    REGION | DESKTOP | WINDOW => {
                        EnableWindow(control, i32::from(idle));
                        ShowWindow(control, visibility);
                    }
                    COPY => {
                        EnableWindow(control, i32::from(preview));
                        ShowWindow(control, visibility);
                    }
                    OPEN => {
                        EnableWindow(control, i32::from(can_open));
                        ShowWindow(control, visibility);
                    }
                    SETTINGS => {
                        let label = if settings_page {
                            "Назад к снимкам"
                        } else {
                            "Настройки"
                        };
                        SetWindowTextW(control, wide(label).as_ptr());
                        InvalidateRect(control, ptr::null(), 0);
                    }
                    CLOSE_TO_TRAY | CLOSE_TO_EXIT => {
                        ShowWindow(control, if settings_page { SW_SHOWNA } else { SW_HIDE });
                        InvalidateRect(control, ptr::null(), 0);
                    }
                    CHOOSE_FOLDER | RESET_FOLDER => {
                        EnableWindow(
                            control,
                            i32::from(idle && (id != RESET_FOLDER || custom_folder)),
                        );
                        ShowWindow(control, if settings_page { SW_SHOWNA } else { SW_HIDE });
                        InvalidateRect(control, ptr::null(), 0);
                    }
                    HISTORY.. => {
                        if !settings_page && let Some(label) = history.get(id - HISTORY) {
                            SetWindowTextW(control, wide(label).as_ptr());
                            ShowWindow(control, SW_SHOWNA);
                            InvalidateRect(control, ptr::null(), 0);
                        } else {
                            ShowWindow(control, SW_HIDE);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    pub fn paint(&self, hwnd: HWND, view: Dashboard<'_>) {
        unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut client = rect(0, 0, 0, 0);
            GetClientRect(hwnd, &mut client);
            let memory = CreateCompatibleDC(dc);
            let bitmap = if memory.is_null() || client.right <= 0 || client.bottom <= 0 {
                ptr::null_mut()
            } else {
                CreateCompatibleBitmap(dc, client.right, client.bottom)
            };
            if bitmap.is_null() {
                self.draw(dc, client, view);
            } else {
                let old = SelectObject(memory, bitmap);
                self.draw(memory, client, view);
                BitBlt(dc, 0, 0, client.right, client.bottom, memory, 0, 0, SRCCOPY);
                SelectObject(memory, old);
                DeleteObject(bitmap);
            }
            if !memory.is_null() {
                DeleteDC(memory);
            }
            EndPaint(hwnd, &ps);
        }
    }

    fn draw(&self, dc: HDC, client: RECT, view: Dashboard<'_>) {
        let s = self.scale;
        let f = &self.fonts;
        fill(dc, BG, client);
        fill(dc, HEADER, rect(0, 0, client.right, s.px(110)));
        fill(dc, EDGE, rect(0, s.px(110), client.right, 1));
        fill(dc, DARK_TEAL, s.rect(32, 30, 54, 54));
        icon(dc, REGION, s.px(47), s.px(45), s.px(24), ACCENT);
        write(
            dc,
            "RustShot",
            s.rect(101, 24, 280, 44),
            TEXT,
            f.brand,
            LINE,
        );
        write(
            dc,
            "Снимки экрана  /  Только локально",
            s.rect(103, 68, 400, 19),
            MUTED,
            f.normal,
            LINE,
        );
        if view.settings_page {
            self.draw_settings(dc, view.save_dir);
        } else {
            self.draw_capture(dc, &view);
        }

        fill(dc, EDGE, s.rect(32, 571, 836, 1));
        let status_color = if view.warn { WARN } else { ACCENT };
        fill(dc, status_color, s.rect(32, 586, 6, 6));
        write(
            dc,
            view.status,
            s.rect(47, 579, 610, 20),
            if view.warn { WARN } else { MUTED },
            f.small,
            LINE,
        );
        let hotkeys = if view.hotkeys == 3 {
            "3 горячие клавиши".to_owned()
        } else {
            format!("Горячие клавиши: {}/3", view.hotkeys)
        };
        write(
            dc,
            &hotkeys,
            s.rect(660, 579, 208, 20),
            if view.hotkeys == 3 { MUTED } else { WARN },
            f.small,
            LINE | DT_RIGHT,
        );
    }

    fn draw_settings(&self, dc: HDC, save_dir: &Path) {
        let s = self.scale;
        let f = &self.fonts;
        write(
            dc,
            "НАСТРОЙКИ",
            s.rect(32, 122, 300, 22),
            ACCENT,
            f.small,
            LINE,
        );
        let panel = s.rect(32, 153, 836, 252);
        fill(dc, PANEL, panel);
        border(dc, EDGE, panel);
        write(
            dc,
            "При нажатии на крестик",
            s.rect(56, 174, 788, 27),
            TEXT,
            f.medium,
            LINE,
        );
        write(
            dc,
            "Выберите, будет ли RustShot работать после закрытия окна.",
            s.rect(56, 207, 788, 21),
            MUTED,
            f.normal,
            LINE,
        );
        write(
            dc,
            "Чтобы завершить работу из трея, нажмите правой кнопкой по значку и выберите «Выход».",
            s.rect(56, 349, 788, 22),
            MUTED,
            f.normal,
            LINE,
        );
        let folder = s.rect(32, 419, 836, 132);
        fill(dc, PANEL, folder);
        border(dc, EDGE, folder);
        write(
            dc,
            "Папка сохранения",
            s.rect(56, 431, 788, 25),
            TEXT,
            f.medium,
            LINE,
        );
        write(
            dc,
            &save_dir.to_string_lossy(),
            s.rect(56, 464, 788, 23),
            MUTED,
            f.normal,
            DT_SINGLELINE | DT_VCENTER | DT_PATH_ELLIPSIS,
        );
    }

    fn draw_capture(&self, dc: HDC, view: &Dashboard<'_>) {
        let s = self.scale;
        let f = &self.fonts;
        write(
            dc,
            "НОВЫЙ СНИМОК",
            s.rect(32, 122, 300, 22),
            ACCENT,
            f.small,
            LINE,
        );

        for panel in [s.rect(32, 245, 554, 306), s.rect(602, 245, 266, 306)] {
            fill(dc, PANEL, panel);
            border(dc, EDGE, panel);
        }
        write(
            dc,
            "Предпросмотр",
            s.rect(49, 259, 340, 27),
            TEXT,
            f.medium,
            LINE,
        );
        write(
            dc,
            "Недавние",
            s.rect(617, 259, 232, 27),
            TEXT,
            f.medium,
            LINE,
        );
        let preview = s.rect(49, 302, 520, 187);
        fill(dc, SURFACE, preview);
        border(dc, EDGE, preview);
        if let Some(frame) = view.preview {
            let dst = frame.fit(Area {
                x: preview.left + 1,
                y: preview.top + 1,
                w: (preview.right - preview.left - 2).max(1),
                h: (preview.bottom - preview.top - 2).max(1),
            });
            unsafe {
                SetStretchBltMode(dc, HALFTONE);
                SetBrushOrgEx(dc, 0, 0, ptr::null_mut());
            }
            frame.draw(dc, dst);
            write(
                dc,
                &format!("{} × {}  ·  PNG", frame.w, frame.h),
                s.rect(353, 264, 215, 18),
                MUTED,
                f.small,
                LINE | DT_RIGHT,
            );
        } else if view.preview_loading || view.history_loading {
            write(
                dc,
                "Загрузка снимка…",
                preview,
                MUTED,
                f.normal,
                LINE | DT_CENTER,
            );
        } else {
            icon(dc, DESKTOP, s.px(297), s.px(347), s.px(24), MUTED);
            write(
                dc,
                "Здесь появится ваш снимок",
                s.rect(57, 383, 504, 23),
                TEXT,
                f.normal,
                LINE | DT_CENTER,
            );
            write(
                dc,
                "Выберите режим захвата выше",
                s.rect(57, 408, 504, 22),
                MUTED,
                f.small,
                LINE | DT_CENTER,
            );
        }
        if view.history_len == 0 {
            write(
                dc,
                if view.history_loading {
                    "Загрузка истории…"
                } else {
                    "Сохранённых снимков пока нет."
                },
                s.rect(617, 301, 236, 22),
                MUTED,
                f.normal,
                LINE,
            );
        }
    }

    pub fn draw_button(
        &self,
        item: &DRAWITEMSTRUCT,
        selected: Option<usize>,
        hotkeys: Hotkeys,
        close_action: CloseAction,
    ) {
        let id = item.CtlID as usize;
        let dc = item.hDC;
        let bounds = item.rcItem;
        let (x, y, w, h) = (
            bounds.left,
            bounds.top,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
        );
        let s = self.scale;
        let f = &self.fonts;
        let active = match id {
            CLOSE_TO_TRAY => close_action == CloseAction::Tray,
            CLOSE_TO_EXIT => close_action == CloseAction::Exit,
            HISTORY.. => selected == Some(id - HISTORY),
            _ => false,
        };
        let pressed = item.itemState & ODS_SELECTED != 0;
        let disabled = item.itemState & ODS_DISABLED != 0;
        let focused = item.itemState & ODS_FOCUS != 0;
        let color = if disabled { MUTED } else { TEXT };
        let fill_color = if disabled {
            SURFACE
        } else if pressed || active || id == REGION {
            DARK_TEAL
        } else {
            PANEL
        };
        fill(dc, fill_color, bounds);
        border(
            dc,
            if focused || active || (id == REGION && !disabled) {
                ACCENT
            } else {
                EDGE
            },
            bounds,
        );

        let mut label = [0u16; 256];
        let len = unsafe { GetWindowTextW(item.hwndItem, label.as_mut_ptr(), label.len() as i32) };
        let label = String::from_utf16_lossy(&label[..len as usize]);
        match id {
            REGION | DESKTOP | WINDOW => {
                let symbol_color = if disabled { MUTED } else { ACCENT };
                icon(dc, id, x + s.px(19), y + s.px(22), s.px(24), symbol_color);
                write(
                    dc,
                    &label,
                    rect(x + s.px(58), y + s.px(11), w - s.px(70), s.px(25)),
                    color,
                    f.medium,
                    LINE,
                );
                let mode = match id {
                    REGION => Mode::Region,
                    DESKTOP => Mode::Desktop,
                    _ => Mode::Window,
                };
                let available = hotkeys.includes(mode);
                let suffix = if !available {
                    "  ·  занято"
                } else if matches!(mode, Mode::Window) {
                    "  ·  кнопка: 3 с"
                } else {
                    ""
                };
                write(
                    dc,
                    &format!("{}{suffix}", mode.label()),
                    rect(x + s.px(58), y + s.px(38), w - s.px(70), s.px(19)),
                    if available { MUTED } else { WARN },
                    f.small,
                    LINE,
                );
            }
            CLOSE_TO_TRAY | CLOSE_TO_EXIT => {
                let left = x + s.px(16);
                write(
                    dc,
                    &label,
                    rect(left, y + s.px(10), w - s.px(32), s.px(27)),
                    color,
                    f.medium,
                    LINE,
                );
                let detail = if id == CLOSE_TO_TRAY {
                    "Горячие клавиши останутся активны"
                } else {
                    "RustShot завершит работу"
                };
                write(
                    dc,
                    detail,
                    rect(left, y + s.px(44), w - s.px(32), s.px(22)),
                    MUTED,
                    f.small,
                    LINE,
                );
                if active {
                    write(
                        dc,
                        "ВЫБРАНО",
                        rect(x + s.px(253), y + s.px(13), w - s.px(269), s.px(20)),
                        ACCENT,
                        f.small,
                        LINE | DT_RIGHT,
                    );
                }
            }
            HISTORY.. => {
                fill(
                    dc,
                    if active { ACCENT } else { EDGE },
                    rect(x + s.px(10), y + s.px(13), s.px(5).max(1), s.px(5).max(1)),
                );
                write(
                    dc,
                    &label,
                    rect(x + s.px(24), y, w - s.px(33), h),
                    color,
                    f.small,
                    LINE,
                );
            }
            _ => write(
                dc,
                &label,
                rect(x + s.px(4), y, w - s.px(8), h),
                color,
                f.normal,
                LINE | DT_CENTER,
            ),
        }
    }
}

fn icon(dc: HDC, id: usize, x: i32, y: i32, size: i32, color: u32) {
    let t = (size / 12).max(1);
    let seg = (size / 3).max(1);
    match id {
        REGION => {
            for &ox in &[0, size - seg] {
                for &oy in &[0, size - t] {
                    fill(dc, color, rect(x + ox, y + oy, seg, t));
                }
            }
            for &ox in &[0, size - t] {
                for &oy in &[0, size - seg] {
                    fill(dc, color, rect(x + ox, y + oy, t, seg));
                }
            }
            fill(
                dc,
                color,
                rect(x + size / 2 - t, y + size / 2 - t, t * 2, t * 2),
            );
        }
        DESKTOP => {
            let height = size - 3 * t;
            fill(dc, color, rect(x, y, size, t));
            fill(dc, color, rect(x, y, t, height));
            fill(dc, color, rect(x + size - t, y, t, height));
            fill(dc, color, rect(x, y + height - t, size, t));
            fill(dc, color, rect(x + size / 2 - t / 2, y + height, t, 2 * t));
            fill(dc, color, rect(x + size / 4, y + size - t, size / 2, t));
        }
        _ => {
            fill(dc, color, rect(x, y + t, size, t));
            fill(dc, color, rect(x, y + t, t, size - 2 * t));
            fill(dc, color, rect(x + size - t, y + t, t, size - 2 * t));
            fill(dc, color, rect(x, y + size - 2 * t, size, t));
            fill(dc, color, rect(x, y + 5 * t, size, t));
            fill(dc, color, rect(x + 2 * t, y + 3 * t, t, t));
        }
    }
}
