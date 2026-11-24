use std::{io, path::PathBuf, sync::mpsc, thread};

use windows_sys::Win32::{Foundation::HWND, UI::WindowsAndMessaging::PostMessageW};

use crate::{HISTORY_READY, frame::Frame, storage, ui::SLOTS};

pub enum Request {
    List(PathBuf),
    Preview(PathBuf),
}

pub enum Loaded {
    List(Vec<PathBuf>),
    Preview(io::Result<Frame>),
}

pub struct Worker {
    pub tx: mpsc::Sender<(u64, Request)>,
    pub rx: mpsc::Receiver<(u64, Loaded)>,
}

impl Worker {
    pub fn start(hwnd: HWND) -> io::Result<Self> {
        let (tx, incoming) = mpsc::channel();
        let (outgoing, rx) = mpsc::channel();
        let handle = hwnd as usize;
        thread::Builder::new()
            .name("rustshot-history".into())
            .spawn(move || {
                while let Ok(first) = incoming.recv() {
                    // Rapid clicks may queue new choices while a PNG is decoding.
                    let (generation, request) = incoming.try_iter().last().unwrap_or(first);
                    let loaded = match request {
                        Request::List(dir) => Loaded::List(storage::history(&dir, SLOTS)),
                        Request::Preview(path) => Loaded::Preview(storage::load(&path)),
                    };
                    if outgoing.send((generation, loaded)).is_err() {
                        break;
                    }
                    unsafe { PostMessageW(handle as HWND, HISTORY_READY, 0, 0) };
                }
            })?;
        Ok(Self { tx, rx })
    }
}

pub fn merge(saved: &mut Vec<PathBuf>, listed: Vec<PathBuf>) {
    for path in listed {
        if saved.len() == SLOTS {
            break;
        }
        if !saved.contains(&path) {
            saved.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listing_keeps_new_captures_first_without_duplicates() {
        let mut saved = vec![PathBuf::from("new")];
        let mut listed = vec![PathBuf::from("new")];
        listed.extend((0..SLOTS).map(|i| PathBuf::from(i.to_string())));
        merge(&mut saved, listed);
        assert_eq!(saved.len(), SLOTS);
        assert_eq!(saved[0], PathBuf::from("new"));
        assert_eq!(saved[1], PathBuf::from("0"));
        assert_eq!(saved[SLOTS - 1], PathBuf::from((SLOTS - 2).to_string()));
    }
}
