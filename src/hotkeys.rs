use windows_sys::Win32::{
    Foundation::HWND,
    UI::Input::KeyboardAndMouse::{
        MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey, VK_SNAPSHOT,
    },
};

#[derive(Clone, Copy)]
pub enum Mode {
    Region,
    Desktop,
    Window,
}

impl Mode {
    pub const ALL: [Self; 3] = [Self::Region, Self::Desktop, Self::Window];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Region => "Ctrl + Print Screen",
            Self::Desktop => "Print Screen",
            Self::Window => "Alt + Print Screen",
        }
    }

    const fn modifiers(self) -> u32 {
        match self {
            Self::Region => MOD_CONTROL,
            Self::Desktop => 0,
            Self::Window => MOD_ALT,
        }
    }

    const fn id(self) -> i32 {
        self as i32 + 1
    }
}

#[derive(Clone, Copy)]
pub struct Hotkeys([bool; Mode::ALL.len()]);

impl Hotkeys {
    pub fn register(hwnd: HWND) -> Self {
        Self(Mode::ALL.map(|mode| unsafe {
            RegisterHotKey(
                hwnd,
                mode.id(),
                mode.modifiers() | MOD_NOREPEAT,
                VK_SNAPSHOT.into(),
            ) != 0
        }))
    }

    pub fn includes(self, mode: Mode) -> bool {
        self.0[mode as usize]
    }

    pub fn count(self) -> usize {
        self.0.into_iter().filter(|&registered| registered).count()
    }

    pub fn pressed(self, id: usize) -> Option<Mode> {
        Mode::ALL
            .into_iter()
            .find(|&mode| mode.id() as usize == id && self.includes(mode))
    }

    pub fn unregister(self, hwnd: HWND) {
        for mode in Mode::ALL.into_iter().filter(|&mode| self.includes(mode)) {
            unsafe { UnregisterHotKey(hwnd, mode.id()) };
        }
    }
}
