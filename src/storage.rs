use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    ptr,
};

use windows_sys::Win32::{
    Foundation::{GlobalFree, HWND, LPARAM, MAX_PATH, SYSTEMTIME},
    System::{
        Com::CoTaskMemFree,
        DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData},
        Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock},
        Ole::{CF_DIB, OleInitialize, OleUninitialize},
        SystemInformation::GetLocalTime,
    },
    UI::{
        Shell::*,
        WindowsAndMessaging::{SW_SHOWNORMAL, SendMessageW},
    },
};

use crate::frame::Frame;

pub fn wide(text: impl AsRef<OsStr>) -> Vec<u16> {
    text.as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

pub fn pictures() -> PathBuf {
    let mut name = ptr::null_mut();
    unsafe {
        if SHGetKnownFolderPath(&FOLDERID_Pictures, 0, ptr::null_mut(), &mut name) >= 0
            && !name.is_null()
        {
            let length = (0..).take_while(|&i| *name.add(i) != 0).count();
            let path = OsString::from_wide(std::slice::from_raw_parts(name, length));
            CoTaskMemFree(name.cast());
            return PathBuf::from(path).join("RustShot");
        }
        if !name.is_null() {
            CoTaskMemFree(name.cast());
        }
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RustShot")
}

pub fn choose_directory(hwnd: HWND, initial: &Path) -> io::Result<Option<PathBuf>> {
    unsafe extern "system" fn initialize(
        hwnd: HWND,
        message: u32,
        _parameter: LPARAM,
        initial: LPARAM,
    ) -> i32 {
        if message == BFFM_INITIALIZED {
            unsafe { SendMessageW(hwnd, BFFM_SETSELECTIONW, 1, initial) };
        }
        0
    }

    let initial = wide(initial);
    let title = wide("Выберите папку для сохранения снимков RustShot");
    let mut display_name = [0u16; MAX_PATH as usize];
    let browse = BROWSEINFOW {
        hwndOwner: hwnd,
        pidlRoot: ptr::null_mut(),
        pszDisplayName: display_name.as_mut_ptr(),
        lpszTitle: title.as_ptr(),
        ulFlags: BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE | BIF_EDITBOX,
        lpfn: Some(initialize),
        lParam: initial.as_ptr() as LPARAM,
        iImage: 0,
    };
    let mut path = vec![0u16; 32_768];
    unsafe {
        let status = OleInitialize(ptr::null());
        if status < 0 {
            return Err(io::Error::other(format!("OLE: 0x{:08X}", status as u32)));
        }
        let item = SHBrowseForFolderW(&browse);
        let result = if item.is_null() {
            Ok(None)
        } else {
            let found =
                SHGetPathFromIDListEx(item, path.as_mut_ptr(), path.len() as u32, GPFIDL_DEFAULT);
            CoTaskMemFree(item.cast());
            let end = path.iter().position(|&unit| unit == 0).unwrap_or(0);
            if found != 0 && end > 0 {
                Ok(Some(OsString::from_wide(&path[..end]).into()))
            } else {
                Err(io::Error::other(
                    "Выбранный объект не является папкой файловой системы",
                ))
            }
        };
        OleUninitialize();
        result
    }
}

pub fn open(hwnd: HWND, path: &Path) -> io::Result<()> {
    let verb = wide("open");
    let path = wide(path);
    let result = unsafe {
        ShellExecuteW(
            hwnd,
            verb.as_ptr(),
            path.as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize > 32 {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "код оболочки Windows: {}",
            result as isize
        )))
    }
}

pub fn history(dir: &Path, limit: usize) -> Vec<PathBuf> {
    let Ok(dir) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<_> = dir
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with("RustShot_") || !name.ends_with(".png") {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            metadata.is_file().then(|| (metadata.modified().ok(), path))
        })
        .collect();
    files.sort_unstable_by(|a, b| b.cmp(a));
    files
        .into_iter()
        .take(limit)
        .map(|(_, path)| path)
        .collect()
}

pub fn label(path: &Path) -> String {
    path.file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("Снимок")
        .trim_start_matches("RustShot_")
        .replace('_', "  ·  ")
        .to_owned()
}

pub fn save(dir: &Path, frame: &Frame) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let mut now: SYSTEMTIME = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut now) };
    let stamp = format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}-{:03}",
        now.wYear, now.wMonth, now.wDay, now.wHour, now.wMinute, now.wSecond, now.wMilliseconds,
    );
    for index in 0..100 {
        let suffix = if index == 0 {
            String::new()
        } else {
            format!("-{index}")
        };
        let path = dir.join(format!("RustShot_{stamp}{suffix}.png"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                if let Err(error) = encode(file, frame) {
                    let _ = fs::remove_file(&path);
                    return Err(error);
                }
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "Не удалось выбрать имя файла",
    ))
}

fn encode(file: File, frame: &Frame) -> io::Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(file), frame.w as u32, frame.h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(io::Error::other)?;
    let mut stream = writer.stream_writer().map_err(io::Error::other)?;
    let mut row = vec![0; frame.w as usize * 3];
    use std::io::Write;
    for source in frame.pixels.chunks_exact(frame.w as usize * 4) {
        for (bgr, rgb) in source
            .as_chunks::<4>()
            .0
            .iter()
            .zip(row.as_chunks_mut::<3>().0)
        {
            *rgb = [bgr[2], bgr[1], bgr[0]];
        }
        stream.write_all(&row)?;
    }
    stream.finish().map_err(io::Error::other)?;
    writer.finish().map_err(io::Error::other)
}

pub fn load(path: &Path) -> io::Result<Frame> {
    let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(io::Error::other)?;
    let (w, h) = (reader.info().width as i32, reader.info().height as i32);
    let bytes = Frame::byte_len(w, h).ok_or_else(|| io::Error::other("Снимок слишком большой"))?;
    let mut raw = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut raw).map_err(io::Error::other)?;
    let mut pixels = Vec::with_capacity(bytes);
    use png::ColorType::*;
    match info.color_type {
        Rgb | Rgba => {
            for p in raw[..info.buffer_size()].chunks_exact(info.color_type.samples()) {
                pixels.extend_from_slice(&[p[2], p[1], p[0], 0]);
            }
        }
        Grayscale | GrayscaleAlpha => {
            for p in raw[..info.buffer_size()].chunks_exact(info.color_type.samples()) {
                pixels.extend_from_slice(&[p[0], p[0], p[0], 0]);
            }
        }
        Indexed => return Err(io::Error::other("Не удалось преобразовать палитру PNG")),
    }
    if pixels.len() != bytes {
        return Err(io::Error::other("Некорректный размер изображения"));
    }
    Ok(Frame { w, h, pixels })
}

fn dib_stride(w: i32) -> usize {
    (w as usize * 3 + 3) & !3
}

fn write_dib(frame: &Frame, output: &mut [u8]) {
    let stride = dib_stride(frame.w);
    let size = stride * frame.h as usize;
    output[..40].fill(0);
    output[0..4].copy_from_slice(&40u32.to_le_bytes());
    output[4..8].copy_from_slice(&frame.w.to_le_bytes());
    output[8..12].copy_from_slice(&frame.h.to_le_bytes());
    output[12..14].copy_from_slice(&1u16.to_le_bytes());
    output[14..16].copy_from_slice(&24u16.to_le_bytes());
    output[20..24].copy_from_slice(&(size as u32).to_le_bytes());
    for (source, dest) in frame
        .pixels
        .chunks_exact(frame.w as usize * 4)
        .rev()
        .zip(output[40..].chunks_exact_mut(stride))
    {
        dest.fill(0);
        let pixels = &mut dest[..frame.w as usize * 3];
        for (bgrx, bgr) in source
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pixels.as_chunks_mut::<3>().0)
        {
            *bgr = [bgrx[0], bgrx[1], bgrx[2]];
        }
    }
}

pub fn copy(hwnd: HWND, frame: &Frame) -> io::Result<()> {
    let size = 40 + dib_stride(frame.w) * frame.h as usize;
    unsafe {
        let handle = GlobalAlloc(GMEM_MOVEABLE, size);
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let bits = GlobalLock(handle);
        if bits.is_null() {
            let error = io::Error::last_os_error();
            GlobalFree(handle);
            return Err(error);
        }
        write_dib(frame, std::slice::from_raw_parts_mut(bits.cast(), size));
        GlobalUnlock(handle);
        if OpenClipboard(hwnd) == 0 {
            let error = io::Error::last_os_error();
            GlobalFree(handle);
            return Err(error);
        }
        let result = if EmptyClipboard() != 0 && !SetClipboardData(CF_DIB as u32, handle).is_null()
        {
            Ok(()) // Windows owns the handle now.
        } else {
            let error = io::Error::last_os_error();
            GlobalFree(handle);
            Err(error)
        };
        CloseClipboard();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_has_bottom_up_order_and_zero_padding() {
        let image = Frame {
            w: 1,
            h: 2,
            pixels: vec![1, 2, 3, 99, 4, 5, 6, 99],
        };
        let mut output = vec![0xff; 48];
        write_dib(&image, &mut output);
        assert_eq!(&output[14..16], &24u16.to_le_bytes());
        assert_eq!(&output[40..], &[4, 5, 6, 0, 1, 2, 3, 0]);
    }

    #[test]
    fn png_round_trip_preserves_color_and_orientation() {
        let dir = std::env::temp_dir().join(format!(
            "rustshot-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let image = Frame {
            w: 2,
            h: 2,
            pixels: (0..16).collect(),
        };
        let path = save(&dir, &image).unwrap();
        let next_path = save(&dir, &image).unwrap();
        let restored = load(&path).unwrap();
        assert_eq!((restored.w, restored.h), (2, 2));
        assert_eq!(
            restored.pixels,
            vec![0, 1, 2, 0, 4, 5, 6, 0, 8, 9, 10, 0, 12, 13, 14, 0]
        );
        assert_ne!(path, next_path);
        assert_eq!(history(&dir, 5).len(), 2);
        fs::remove_dir_all(dir).unwrap();
    }
}
