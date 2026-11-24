use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CloseAction {
    #[default]
    Tray,
    Exit,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub close_action: CloseAction,
    pub save_dir: Option<PathBuf>,
}

impl Settings {
    pub fn load(path: &Path) -> io::Result<Self> {
        let data = match fs::read_to_string(path) {
            Ok(data) => data,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(error) => return Err(error),
        };
        let mut settings = Self::default();
        for (key, value) in data.lines().filter_map(|line| line.split_once('=')) {
            match key {
                "close_action" => {
                    settings.close_action = match value.trim() {
                        "tray" => CloseAction::Tray,
                        "exit" => CloseAction::Exit,
                        _ => return Err(invalid("неизвестное значение параметра закрытия")),
                    };
                }
                "save_dir" => settings.save_dir = Some(directory(value)?.into()),
                _ => {}
            }
        }
        Ok(settings)
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let mut data = match self.close_action {
            CloseAction::Tray => String::from("close_action=tray\n"),
            CloseAction::Exit => String::from("close_action=exit\n"),
        };
        if let Some(dir) = &self.save_dir {
            let value = dir
                .to_str()
                .ok_or_else(|| invalid("неподдерживаемые символы в пути папки"))?;
            directory(value)?;
            data.push_str("save_dir=");
            data.push_str(value);
            data.push('\n');
        }
        if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let pending = path.with_extension(format!("{}.tmp", std::process::id()));
        let result = fs::write(&pending, data).and_then(|()| fs::rename(&pending, path));
        if result.is_err() {
            let _ = fs::remove_file(&pending);
        }
        result
    }
}

fn directory(value: &str) -> io::Result<&Path> {
    let path = Path::new(value);
    if !path.is_absolute() || value.contains(['\r', '\n', '\0']) {
        return Err(invalid("путь папки сохранения должен быть абсолютным"));
    }
    Ok(path)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn path() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("RustShot")
        .join("settings.ini")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_legacy_settings_and_saves_both_options_independently() -> io::Result<()> {
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("rustshot-settings-{}-{id}", std::process::id()));
        let path = dir.join("settings.ini");
        assert_eq!(Settings::load(&path)?, Settings::default());
        fs::create_dir_all(&dir)?;
        fs::write(&path, "close_action=exit\r\n")?;
        let mut settings = Settings::load(&path)?;
        assert_eq!(settings.close_action, CloseAction::Exit);
        assert_eq!(settings.save_dir, None);

        settings.save_dir = Some(dir.join("Мои снимки = PNG"));
        settings.save(&path)?;
        assert_eq!(Settings::load(&path)?, settings);
        settings.close_action = CloseAction::Tray;
        settings.save(&path)?;
        assert_eq!(Settings::load(&path)?, settings);
        settings.save_dir = None;
        settings.save(&path)?;
        assert_eq!(Settings::load(&path)?, Settings::default());
        fs::remove_file(path)?;
        fs::remove_dir(dir)?;
        Ok(())
    }

    #[test]
    fn rejects_relative_paths_and_line_breaks() {
        assert!(directory("relative").is_err());
        assert!(directory("C:\\first\nclose_action=exit").is_err());
    }
}
