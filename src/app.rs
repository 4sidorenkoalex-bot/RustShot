use std::{
    cell::RefCell,
    fs, io,
    path::PathBuf,
    ptr,
    sync::{Arc, mpsc},
    thread,
};

use windows_sys::Win32::{
    Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::{Dwm::DwmSetWindowAttribute, Gdi::*},
    System::LibraryLoader::GetModuleHandleW,
    UI::{Controls::*, HiDpi::*, Shell::*, WindowsAndMessaging::*},
};

use crate::{
    EXIT_REQUEST, HISTORY_READY, REGION_READY, SAVE_READY, TRAY_EVENT,
    frame::{self, Area, Frame},
    history::{self, Loaded, Request},
    hotkeys::{Hotkeys, Mode},
    overlay,
    settings::{self, CloseAction, Settings},
    storage::{self, wide},
    ui,
};

const CLASS: &str = "RustShot.Dashboard";
const TIMER: usize = 1;
const TRAY_ID: u32 = 1;
const TRAY_OPEN: usize = 1;
const TRAY_EXIT: usize = 2;
thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

enum Phase {
    Idle,
    Delay(Mode),
    Selecting,
    Saving,
    PickingFolder,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    History,
    Preview,
}

struct Completed {
    result: io::Result<PathBuf>,
    frame: Arc<Frame>,
    copied: bool,
}

struct App {
    hwnd: HWND,
    instance: HINSTANCE,
    icon: HICON,
    ui: ui::Ui,
    default_dir: PathBuf,
    dir: PathBuf,
    history: Vec<PathBuf>,
    current: Option<PathBuf>,
    preview: Option<Arc<Frame>>,
    phase: Phase,
    restore: Option<bool>,
    close_after_save: bool,
    settings: Settings,
    settings_path: PathBuf,
    settings_page: bool,
    status: String,
    warn: bool,
    hotkeys: Hotkeys,
    tray: bool,
    tray_hint_shown: bool,
    taskbar_created: u32,
    tx: mpsc::Sender<Completed>,
    rx: mpsc::Receiver<Completed>,
    browser: history::Worker,
    generation: u64,
    pending: Option<Pending>,
}

impl App {
    fn new(hwnd: HWND, instance: HINSTANCE, icon: HICON) -> io::Result<Self> {
        let default_dir = storage::pictures();
        let (tx, rx) = mpsc::channel();
        let settings_path = settings::path();
        let loaded_settings = Settings::load(&settings_path);
        let settings = loaded_settings.as_ref().cloned().unwrap_or_default();
        let dir = settings
            .save_dir
            .clone()
            .unwrap_or_else(|| default_dir.clone());
        let mut app = Self {
            hwnd,
            instance,
            icon,
            ui: ui::Ui::new(hwnd)?,
            default_dir,
            dir,
            history: Vec::new(),
            current: None,
            preview: None,
            phase: Phase::Idle,
            restore: None,
            close_after_save: false,
            settings,
            settings_path,
            settings_page: false,
            status: "Готово к захвату".into(),
            warn: false,
            hotkeys: Hotkeys::register(hwnd),
            tray: false,
            tray_hint_shown: false,
            taskbar_created: unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
            tx,
            rx,
            browser: history::Worker::start(hwnd)?,
            generation: 0,
            pending: None,
        };
        if app.hotkeys.count() != Mode::ALL.len() {
            app.status = "Не все горячие клавиши доступны. Кнопки захвата работают.".into();
            app.warn = true;
        }
        if let Err(error) = loaded_settings {
            app.status = format!("Не удалось загрузить настройки: {error}");
            app.warn = true;
        }
        app.reload_history();
        app.add_tray();
        app.refresh();
        Ok(app)
    }

    fn idle(&self) -> bool {
        matches!(self.phase, Phase::Idle)
    }

    fn selected(&self) -> Option<usize> {
        self.current
            .as_ref()
            .and_then(|path| self.history.iter().position(|entry| entry == path))
    }

    fn refresh(&self) {
        let labels: Vec<_> = self
            .history
            .iter()
            .map(|path| storage::label(path))
            .collect();
        self.ui.update(
            self.idle(),
            self.preview.is_some(),
            self.current.is_some(),
            &labels,
            self.settings_page,
            self.settings.save_dir.is_some(),
        );
        unsafe { InvalidateRect(self.hwnd, ptr::null(), 0) };
    }

    fn report(&mut self, text: impl Into<String>, warn: bool, balloon: bool) {
        self.status = text.into();
        self.warn = warn;
        self.refresh();
        if self.tray
            && balloon
            && unsafe { IsWindowVisible(self.hwnd) == 0 || IsIconic(self.hwnd) != 0 }
        {
            self.notify(&self.status, warn);
        }
    }

    fn notify(&self, text: &str, warn: bool) {
        let mut data = self.tray_data();
        data.uFlags = NIF_INFO;
        data.dwInfoFlags = if warn { NIIF_WARNING } else { NIIF_INFO };
        put_wide(&mut data.szInfoTitle, "RustShot");
        put_wide(&mut data.szInfo, text);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &data) };
    }

    fn set_close_action(&mut self, action: CloseAction) {
        let settings = Settings {
            close_action: action,
            ..self.settings.clone()
        };
        match settings.save(&self.settings_path) {
            Ok(()) => {
                self.settings = settings;
                let status = match action {
                    CloseAction::Tray => "Крестик сворачивает RustShot в трей.",
                    CloseAction::Exit => "Крестик полностью закрывает RustShot.",
                };
                self.report(status, false, false);
            }
            Err(error) => self.report(
                format!("Не удалось сохранить настройки: {error}"),
                true,
                false,
            ),
        }
    }

    fn set_directory(&mut self, dir: Option<PathBuf>) {
        let settings = Settings {
            save_dir: dir.filter(|dir| dir != &self.default_dir),
            ..self.settings.clone()
        };
        if settings == self.settings {
            self.refresh();
            return;
        }
        match settings.save(&self.settings_path) {
            Ok(()) => {
                self.dir = settings
                    .save_dir
                    .clone()
                    .unwrap_or_else(|| self.default_dir.clone());
                self.settings = settings;
                self.reload_history();
                self.report(
                    "Папка сохранения изменена. Старые снимки остались на месте.",
                    false,
                    false,
                );
            }
            Err(error) => self.report(
                format!("Не удалось сохранить настройки: {error}"),
                true,
                false,
            ),
        }
    }

    fn load(&mut self, request: Request, pending: Pending) {
        self.generation = self.generation.wrapping_add(1);
        self.pending = if self.browser.tx.send((self.generation, request)).is_ok() {
            Some(pending)
        } else {
            self.status = "Фоновая загрузка снимков недоступна.".into();
            self.warn = true;
            None
        };
    }

    fn reload_history(&mut self) {
        self.history.clear();
        self.current = None;
        self.preview = None;
        self.load(Request::List(self.dir.clone()), Pending::History);
    }

    fn select_preview(&mut self, path: PathBuf) {
        self.current = Some(path.clone());
        self.preview = None;
        self.load(Request::Preview(path), Pending::Preview);
    }

    fn history_ready(&mut self) {
        while let Ok((generation, result)) = self.browser.rx.try_recv() {
            if generation != self.generation || self.pending.is_none() {
                continue;
            }
            self.pending = None;
            match result {
                Loaded::List(list) => {
                    history::merge(&mut self.history, list);
                    if self.current.is_none()
                        && self.preview.is_none()
                        && let Some(path) = self.history.first().cloned()
                    {
                        self.select_preview(path);
                    }
                }
                Loaded::Preview(Ok(frame)) => self.preview = Some(Arc::new(frame)),
                Loaded::Preview(Err(error)) => {
                    if error.kind() == io::ErrorKind::NotFound
                        && let Some(path) = self.current.take()
                    {
                        self.history.retain(|entry| entry != &path);
                    }
                    if self.idle() {
                        self.report(format!("Не удалось загрузить снимок: {error}"), true, false);
                        continue;
                    }
                }
            }
            self.refresh();
        }
    }

    fn hide_to_tray(&mut self) {
        self.restore = None;
        unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    fn should_exit(&mut self, force: bool) -> bool {
        if matches!(self.phase, Phase::PickingFolder) {
            return false;
        }
        if !force && self.settings.close_action == CloseAction::Tray && self.tray {
            self.hide_to_tray();
            if !self.tray_hint_shown {
                self.tray_hint_shown = true;
                self.notify("RustShot работает в трее. Для полного закрытия выберите «Выход» в меню значка.", false);
            }
            return false;
        }
        if matches!(self.phase, Phase::Saving) {
            self.close_after_save = true;
            self.restore = None;
            return false;
        }
        true
    }

    fn start(&mut self, mode: Mode, button: bool) {
        if !self.idle() {
            return;
        }
        if matches!(mode, Mode::Window) && !button {
            if let Some(area) = frame::active_window(self.hwnd) {
                self.grab(area, mode);
            } else {
                self.report(
                    "Сначала переключитесь на окно, которое хотите снять.",
                    true,
                    true,
                );
            }
            return;
        }

        self.phase = Phase::Delay(mode);
        self.report(
            if matches!(mode, Mode::Window) {
                "Переключитесь на нужное окно. Съёмка через 3 секунды."
            } else {
                "Подготовка к захвату…"
            },
            false,
            false,
        );
        if unsafe { IsWindowVisible(self.hwnd) != 0 && IsIconic(self.hwnd) == 0 } {
            self.restore = Some(button);
            unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
        let delay = if matches!(mode, Mode::Window) {
            3000
        } else {
            190
        };
        if unsafe { SetTimer(self.hwnd, TIMER, delay, None) } == 0 {
            self.fail("Не удалось запустить таймер", io::Error::last_os_error());
        }
    }

    fn timer(&mut self) {
        unsafe { KillTimer(self.hwnd, TIMER) };
        let Phase::Delay(mode) = self.phase else {
            return;
        };
        let area = if matches!(mode, Mode::Window) {
            frame::active_window(self.hwnd)
        } else {
            Some(frame::desktop())
        };
        if let Some(area) = area {
            self.grab(area, mode);
        } else {
            self.ready();
            let hint = if self.hotkeys.includes(Mode::Window) {
                format!("Переключитесь на него и нажмите {}.", Mode::Window.label())
            } else {
                "Повторите захват после переключения на нужное окно.".into()
            };
            self.report(format!("Не найдено активное окно. {hint}"), true, true);
        }
    }

    fn grab(&mut self, area: Area, mode: Mode) {
        match frame::capture(area) {
            Ok(frame) if matches!(mode, Mode::Region) => {
                self.phase = Phase::Selecting;
                if let Err(error) = overlay::begin(self.hwnd, self.instance, area, frame) {
                    self.fail("Не удалось открыть выбор области", error);
                }
            }
            Ok(frame) => self.save(frame),
            Err(error) => self.fail("Не удалось снять экран", error),
        }
    }

    fn region_ready(&mut self) {
        if !matches!(self.phase, Phase::Selecting) {
            return;
        }
        if let Some(frame) = overlay::take() {
            self.save(frame);
        } else {
            self.ready();
            self.report("Выбор области отменён.", false, false);
        }
    }

    fn save(&mut self, frame: Frame) {
        self.phase = Phase::Saving;
        self.report("Сохранение снимка…", false, false);
        let copied = storage::copy(self.hwnd, &frame).is_ok();
        let frame = Arc::new(frame);
        let image = frame.clone();
        let directory = self.dir.clone();
        let tx = self.tx.clone();
        let handle = self.hwnd as usize;
        if let Err(error) = thread::Builder::new()
            .name("rustshot-png".into())
            .spawn(move || {
                let result = storage::save(&directory, &image);
                if tx
                    .send(Completed {
                        result,
                        frame: image,
                        copied,
                    })
                    .is_ok()
                {
                    unsafe { PostMessageW(handle as HWND, SAVE_READY, 0, 0) };
                }
            })
        {
            self.complete(Completed {
                result: Err(error),
                frame,
                copied,
            });
        }
    }

    fn complete(&mut self, job: Completed) {
        if self.close_after_save {
            self.phase = Phase::Idle;
            self.restore = None;
            unsafe { PostMessageW(self.hwnd, EXIT_REQUEST, 0, 0) };
            return;
        }
        if self.pending == Some(Pending::Preview) {
            self.pending = None;
        }
        self.preview = Some(job.frame);
        self.ready();
        match job.result {
            Ok(path) => {
                self.history.retain(|entry| entry != &path);
                self.history.insert(0, path.clone());
                self.history.truncate(ui::SLOTS);
                self.current = Some(path);
                if job.copied {
                    self.report(
                        "PNG сохранён, снимок скопирован в буфер обмена.",
                        false,
                        true,
                    );
                } else {
                    self.report(
                        "PNG сохранён. Не удалось скопировать снимок — можно повторить.",
                        true,
                        true,
                    );
                }
            }
            Err(error) => {
                self.current = None;
                self.report(
                    if job.copied {
                        format!("Не удалось сохранить PNG; снимок скопирован: {error}")
                    } else {
                        format!("Не удалось сохранить PNG и скопировать снимок: {error}")
                    },
                    true,
                    true,
                );
            }
        }
    }

    fn ready(&mut self) {
        self.phase = Phase::Idle;
        if let Some(activate) = self.restore.take() {
            let command = if activate { SW_SHOW } else { SW_SHOWNOACTIVATE };
            unsafe { ShowWindow(self.hwnd, command) };
        }
    }

    fn fail(&mut self, context: &str, error: io::Error) {
        self.ready();
        self.report(format!("{context}: {error}"), true, true);
    }

    fn command(&mut self, id: usize) {
        match id {
            ui::SETTINGS => {
                self.settings_page = !self.settings_page;
                self.refresh();
            }
            ui::CLOSE_TO_TRAY if self.settings_page => self.set_close_action(CloseAction::Tray),
            ui::CLOSE_TO_EXIT if self.settings_page => self.set_close_action(CloseAction::Exit),
            ui::RESET_FOLDER if self.settings_page && self.idle() => self.set_directory(None),
            ui::REGION => self.start(Mode::Region, true),
            ui::DESKTOP => self.start(Mode::Desktop, true),
            ui::WINDOW => self.start(Mode::Window, true),
            ui::FOLDER => {
                if let Err(error) =
                    fs::create_dir_all(&self.dir).and_then(|()| storage::open(self.hwnd, &self.dir))
                {
                    self.report(format!("Не удалось открыть папку: {error}"), true, true);
                }
            }
            ui::COPY => {
                if let Some(frame) = self.preview.as_ref() {
                    match storage::copy(self.hwnd, frame) {
                        Ok(()) => self.report("Снимок скопирован в буфер обмена.", false, false),
                        Err(error) => self.report(
                            format!("Не удалось скопировать снимок: {error}"),
                            true,
                            false,
                        ),
                    }
                }
            }
            ui::OPEN => {
                if let Some(path) = self.current.as_ref()
                    && let Err(error) = storage::open(self.hwnd, path)
                {
                    self.report(format!("Не удалось открыть PNG: {error}"), true, false);
                }
            }
            ui::HISTORY..
                if self.idle()
                    && self.pending != Some(Pending::History)
                    && let Some(path) = self.history.get(id - ui::HISTORY).cloned() =>
            {
                self.select_preview(path);
                self.refresh();
            }
            _ => {}
        }
    }

    fn tray_data(&self) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_ID,
            ..unsafe { std::mem::zeroed() }
        }
    }

    fn add_tray(&mut self) {
        let mut data = self.tray_data();
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = TRAY_EVENT;
        data.hIcon = self.icon;
        put_wide(&mut data.szTip, "RustShot — снимки экрана");
        self.tray = unsafe { Shell_NotifyIconW(NIM_ADD, &data) } != 0;
    }

    fn show(&mut self) {
        if self.idle() {
            unsafe {
                ShowWindow(self.hwnd, SW_RESTORE);
                SetForegroundWindow(self.hwnd);
            }
            self.ui.layout(self.hwnd);
        }
    }

    fn cleanup(&self) {
        unsafe {
            KillTimer(self.hwnd, TIMER);
            self.hotkeys.unregister(self.hwnd);
            if self.tray {
                Shell_NotifyIconW(NIM_DELETE, &self.tray_data());
            }
        }
    }
}

fn put_wide<const N: usize>(target: &mut [u16; N], text: &str) {
    for (dest, unit) in target[..N - 1].iter_mut().zip(text.encode_utf16()) {
        *dest = unit;
    }
}

fn with_app<T>(f: impl FnOnce(&mut App) -> T) -> Option<T> {
    APP.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut state| state.as_mut().map(f))
    })
}

fn choose_folder(hwnd: HWND) {
    let Some(Some(initial)) = with_app(|app| {
        if !app.settings_page || !app.idle() {
            return None;
        }
        app.phase = Phase::PickingFolder;
        app.refresh();
        Some(app.dir.clone())
    }) else {
        return;
    };
    // The shell dialog pumps messages, so no App borrow may span this call.
    let picked = storage::choose_directory(hwnd, &initial);
    with_app(|app| {
        app.phase = Phase::Idle;
        match picked {
            Ok(Some(dir)) => app.set_directory(Some(dir)),
            Ok(None) => app.refresh(),
            Err(error) => app.report(format!("Не удалось выбрать папку: {error}"), true, false),
        }
    });
}

// A native menu pumps messages. Open it without holding the app's RefCell borrow.
fn track_menu(hwnd: HWND, menu: HMENU, x: i32, y: i32) -> usize {
    unsafe {
        SetForegroundWindow(hwnd);
        let choice = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            x,
            y,
            0,
            hwnd,
            ptr::null(),
        );
        DestroyMenu(menu);
        PostMessageW(hwnd, WM_NULL, 0, 0);
        choice as usize
    }
}

fn tray_event(hwnd: HWND, event: u32) {
    match event {
        WM_LBUTTONDBLCLK => {
            with_app(App::show);
        }
        WM_RBUTTONUP | WM_CONTEXTMENU => {
            let Some(idle) = with_app(|app| app.idle()) else {
                return;
            };
            let choice = unsafe {
                let menu = CreatePopupMenu();
                if menu.is_null() {
                    return;
                }
                for (id, name) in [
                    (TRAY_OPEN, "Открыть RustShot"),
                    (ui::REGION, "Захват области"),
                    (ui::DESKTOP, "Весь экран"),
                    (ui::FOLDER, "Папка снимков"),
                    (TRAY_EXIT, "Выход"),
                ] {
                    let flags = if !idle && matches!(id, TRAY_OPEN | ui::REGION | ui::DESKTOP) {
                        MF_GRAYED
                    } else {
                        MF_STRING
                    };
                    AppendMenuW(menu, flags, id, wide(name).as_ptr());
                }
                let mut pos = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pos);
                track_menu(hwnd, menu, pos.x, pos.y)
            };
            if choice == TRAY_EXIT {
                unsafe { PostMessageW(hwnd, EXIT_REQUEST, 0, 0) };
            } else if choice != 0 {
                with_app(|app| match choice {
                    TRAY_OPEN => app.show(),
                    ui::REGION => app.start(Mode::Region, false),
                    ui::DESKTOP => app.start(Mode::Desktop, false),
                    id => app.command(id),
                });
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            if with_app(|app| {
                app.ui.paint(
                    hwnd,
                    ui::Dashboard {
                        preview: app.preview.as_deref(),
                        preview_loading: app.pending == Some(Pending::Preview),
                        history_loading: app.pending == Some(Pending::History),
                        history_len: app.history.len(),
                        settings_page: app.settings_page,
                        save_dir: &app.dir,
                        status: &app.status,
                        warn: app.warn,
                        hotkeys: app.hotkeys.count(),
                    },
                )
            })
            .is_none()
            {
                unsafe { ValidateRect(hwnd, ptr::null()) };
            }
        }
        WM_ERASEBKGND => return 1,
        WM_DRAWITEM if lparam != 0 => {
            if with_app(|app| {
                let item = unsafe { &*(lparam as *const DRAWITEMSTRUCT) };
                app.ui
                    .draw_button(item, app.selected(), app.hotkeys, app.settings.close_action);
            })
            .is_some()
            {
                return 1;
            }
        }
        WM_SIZE => {
            with_app(|app| {
                if wparam == SIZE_MINIMIZED as usize {
                    if app.tray {
                        app.hide_to_tray();
                    }
                } else {
                    app.ui.layout(hwnd);
                }
            });
        }
        WM_DPICHANGED if lparam != 0 => unsafe {
            let r = &*(lparam as *const windows_sys::Win32::Foundation::RECT);
            SetWindowPos(
                hwnd,
                ptr::null_mut(),
                r.left,
                r.top,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            );
        },
        WM_COMMAND if (wparam >> 16) == BN_CLICKED as usize => {
            let id = wparam & 0xffff;
            if id == ui::CHOOSE_FOLDER {
                choose_folder(hwnd);
            } else {
                with_app(|app| app.command(id));
            }
        }
        WM_HOTKEY => {
            with_app(|app| {
                if let Some(mode) = app.hotkeys.pressed(wparam) {
                    app.start(mode, false);
                }
            });
        }
        WM_TIMER if wparam == TIMER => {
            with_app(App::timer);
        }
        REGION_READY => {
            with_app(App::region_ready);
        }
        SAVE_READY => {
            with_app(|app| {
                if let Ok(job) = app.rx.try_recv() {
                    app.complete(job);
                }
            });
        }
        HISTORY_READY => {
            with_app(App::history_ready);
        }
        TRAY_EVENT if wparam == TRAY_ID as usize => {
            tray_event(hwnd, lparam as u32);
        }
        WM_CLOSE | EXIT_REQUEST => {
            if with_app(|app| app.should_exit(msg == EXIT_REQUEST)).unwrap_or(false) {
                unsafe { DestroyWindow(hwnd) };
            }
        }
        WM_DESTROY => {
            with_app(|app| app.cleanup());
            unsafe { PostQuitMessage(0) };
        }
        _ => {
            let handled = with_app(|app| {
                if msg == app.taskbar_created && msg != 0 {
                    app.add_tray();
                    if !app.tray {
                        app.show();
                    }
                    return true;
                }
                false
            })
            .unwrap_or(false);
            if !handled {
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
        }
    }
    0
}

fn app_icon(instance: HINSTANCE) -> HICON {
    let mut image = [0u8; 32 * 32 * 4];
    for y in 0..32usize {
        for x in 0..32usize {
            let at = (y * 32 + x) * 4;
            let near_x = x.min(31 - x);
            let near_y = y.min(31 - y);
            let edge = ((6..8).contains(&near_x) && (6..13).contains(&near_y))
                || ((6..8).contains(&near_y) && (6..13).contains(&near_x));
            let color = if edge || (near_x >= 14 && near_y >= 14) {
                [205, 235, 111, 255]
            } else if x < 2 || y < 2 || x > 29 || y > 29 {
                [37, 23, 14, 255]
            } else {
                [66, 63, 25, 255]
            };
            image[at..at + 4].copy_from_slice(&color);
        }
    }
    let mask = [0u8; 32 * 4];
    unsafe { CreateIcon(instance, 32, 32, 1, 32, mask.as_ptr(), image.as_ptr()) }
}

pub fn run() -> io::Result<()> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(ptr::null());
        overlay::register(instance)?;
        let class_name = wide(CLASS);
        let owned_icon = app_icon(instance);
        let icon = if owned_icon.is_null() {
            LoadIconW(ptr::null_mut(), IDI_APPLICATION)
        } else {
            owned_icon
        };
        let result = run_window(instance, icon, &class_name);
        if !owned_icon.is_null() {
            DestroyIcon(owned_icon);
        }
        result
    }
}

unsafe fn run_window(instance: HINSTANCE, icon: HICON, class_name: &[u16]) -> io::Result<()> {
    unsafe {
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            hIcon: icon,
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            lpszClassName: class_name.as_ptr(),
            ..std::mem::zeroed()
        };
        if RegisterClassW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        let ex = WS_EX_CONTROLPARENT;
        let dpi = GetDpiForSystem().max(96);
        let mut bounds = ui::rect(
            0,
            0,
            ui::WIDTH * dpi as i32 / 96,
            ui::HEIGHT * dpi as i32 / 96,
        );
        AdjustWindowRectExForDpi(&mut bounds, style, 0, ex, dpi);
        let (mut w, mut h) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..std::mem::zeroed()
        };
        let (mut x, mut y) = (CW_USEDEFAULT, CW_USEDEFAULT);
        if GetMonitorInfoW(monitor, &mut info) != 0 {
            let work = info.rcWork;
            let free_w = work.right - work.left;
            let free_h = work.bottom - work.top;
            let scale = (free_w as f64 / w as f64)
                .min(free_h as f64 / h as f64)
                .min(1.0);
            w = (w as f64 * scale) as i32;
            h = (h as f64 * scale) as i32;
            x = work.left + (free_w - w) / 2;
            y = work.top + (free_h - h) / 2;
        }
        let hwnd = CreateWindowExW(
            ex,
            class_name.as_ptr(),
            wide("RustShot").as_ptr(),
            style,
            x,
            y,
            w,
            h,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if hwnd.is_null() {
            let error = io::Error::last_os_error();
            UnregisterClassW(class_name.as_ptr(), instance);
            return Err(error);
        }
        let dark = 1i32;
        DwmSetWindowAttribute(
            hwnd,
            20,
            (&dark as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        );
        let app = match App::new(hwnd, instance, icon) {
            Ok(app) => app,
            Err(error) => {
                DestroyWindow(hwnd);
                UnregisterClassW(class_name.as_ptr(), instance);
                return Err(error);
            }
        };
        APP.with_borrow_mut(|state| *state = Some(app));
        ShowWindow(hwnd, SW_SHOWNORMAL);
        UpdateWindow(hwnd);
        let mut msg: MSG = std::mem::zeroed();
        let result = loop {
            match GetMessageW(&mut msg, ptr::null_mut(), 0, 0) {
                0 => break Ok(()),
                -1 => break Err(io::Error::last_os_error()),
                _ => {
                    let keyboard = (WM_KEYFIRST..=WM_KEYLAST).contains(&msg.message);
                    if keyboard
                        && (msg.hwnd == hwnd || IsChild(hwnd, msg.hwnd) != 0)
                        && IsDialogMessageW(hwnd, &msg) != 0
                    {
                        continue;
                    }
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        };
        if IsWindow(hwnd) != 0 {
            DestroyWindow(hwnd);
        }
        APP.with_borrow_mut(|state| *state = None);
        UnregisterClassW(class_name.as_ptr(), instance);
        result
    }
}
