use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "config.json";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HotkeyConfig {
    pub keycode: u16,
    /// Spoken name, kept so narration does not depend on a lookup table.
    pub label: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub hotkey: Option<HotkeyConfig>,
    pub setup_complete: bool,
}

impl Config {
    /// Loads the saved config, or defaults when it is missing or unreadable.
    pub fn load(dir: &Path) -> Self {
        fs::read_to_string(dir.join(FILE_NAME))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Writes the config atomically so a crash never leaves a half-written file.
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!("{FILE_NAME}.tmp"));
        fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        fs::rename(tmp, dir.join(FILE_NAME))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("chirp-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = scratch_dir("missing");
        assert_eq!(Config::load(&dir), Config::default());
    }

    #[test]
    fn saved_config_round_trips() {
        let dir = scratch_dir("roundtrip");
        let config = Config {
            hotkey: Some(HotkeyConfig { keycode: 61, label: "Right Option".into() }),
            setup_complete: true,
        };
        config.save(&dir).unwrap();
        assert_eq!(Config::load(&dir), config);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = scratch_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), "{ not json").unwrap();
        assert_eq!(Config::load(&dir), Config::default());
        let _ = fs::remove_dir_all(&dir);
    }
}
