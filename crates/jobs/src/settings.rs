//! User settings (docs/12 §11): one small JSON file in the data dir, `settings.json`.
//!
//! - **Tolerant.** Unknown fields are ignored and missing ones take their defaults, so an
//!   older or newer app reads the file without complaint. `version` records the format.
//! - **Read once, cached.** [`current`] loads the file the first time it's asked for and
//!   then hands out the same `Arc` (a read lock, no I/O), so the overlay thread can check
//!   it cheaply. [`reload`] re-reads it after the Settings window saves; [`save`] writes it
//!   atomically (temp file + rename) and updates the cache.
//! - **Applied where it's used:** [`crate::Options::from_settings`] for conversions,
//!   [`Settings::discovery`] for engine packs, the window hosts for the theme, the agent for
//!   pausing the wheel and starting with Windows.

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};
use swirvo_core::Preset;

/// The format of `settings.json`.
pub const VERSION: u32 = 1;

/// Light, dark, or whatever Windows uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

/// The default quality preset for conversions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PresetChoice {
    Best,
    #[default]
    Balanced,
    Smallest,
}

impl PresetChoice {
    pub fn preset(self) -> Preset {
        match self {
            PresetChoice::Best => Preset::Best,
            PresetChoice::Balanced => Preset::Balanced,
            PresetChoice::Smallest => Preset::Smallest,
        }
    }
}

/// Which engine packs found on this PC may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Discovery {
    pub office: bool,
    pub libreoffice: bool,
    pub ffmpeg: bool,
}

impl Default for Discovery {
    fn default() -> Self {
        Discovery {
            office: true,
            libreoffice: true,
            ffmpeg: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,

    // General
    /// Start Swirvo when you sign in to Windows.
    pub start_with_windows: bool,
    /// Show the "Swirvo is running" tip when the app starts.
    pub startup_tip: bool,
    pub theme: ThemeChoice,
    /// The install offer was shown once (yes or no); don't ask again.
    pub install_offered: bool,

    // Output
    /// None: next to each original (the default). Some: always this folder.
    pub out_dir: Option<PathBuf>,
    pub preset: PresetChoice,
    /// Open the folder of a finished conversion from the wheel.
    pub show_in_folder: bool,

    // Wheel
    /// Ignore the trigger keys until turned back on.
    pub wheel_paused: bool,

    // Engines
    pub use_office: bool,
    pub use_libreoffice: bool,
    pub use_ffmpeg: bool,
    /// Files worked on at once; 0 = one per processor core.
    pub jobs: usize,

    // Privacy
    /// Never use the network. Nothing in the app does yet; future update checks will only
    /// run with this off.
    pub offline: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            version: VERSION,
            start_with_windows: true,
            startup_tip: true,
            theme: ThemeChoice::System,
            install_offered: false,
            out_dir: None,
            preset: PresetChoice::Balanced,
            show_in_folder: false,
            wheel_paused: false,
            use_office: true,
            use_libreoffice: true,
            use_ffmpeg: true,
            jobs: 0,
            offline: true,
        }
    }
}

/// The most files worked on at once that the Settings window offers.
pub const MAX_JOBS: usize = 16;

impl Settings {
    /// Engine packs this PC may use.
    pub fn discovery(&self) -> Discovery {
        Discovery {
            office: self.use_office,
            libreoffice: self.use_libreoffice,
            ffmpeg: self.use_ffmpeg,
        }
    }

    /// Files worked on at once: the setting, or one per core for Auto.
    pub fn jobs(&self) -> usize {
        match self.jobs {
            0 => std::thread::available_parallelism().map_or(4, |n| n.get()),
            n => n.min(MAX_JOBS),
        }
    }

    /// Parse a settings file; anything unreadable gives the defaults.
    pub fn from_json(bytes: &[u8]) -> Settings {
        let mut s: Settings = serde_json::from_slice(bytes).unwrap_or_default();
        s.version = VERSION;
        s.jobs = s.jobs.min(MAX_JOBS);
        // A folder that's gone (a removed USB drive) falls back to "next to the original".
        if s.out_dir.as_ref().is_some_and(|d| d.as_os_str().is_empty()) {
            s.out_dir = None;
        }
        s
    }
}

/// Where the settings live: `<data dir>/settings.json`.
pub fn path() -> Option<PathBuf> {
    crate::data_dir().map(|d| d.join("settings.json"))
}

/// Read the file (defaults when it's missing or unreadable). I/O: not for UI threads.
pub fn load() -> Settings {
    path()
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| Settings::from_json(&b))
        .unwrap_or_default()
}

fn cache() -> &'static RwLock<Arc<Settings>> {
    static CACHE: OnceLock<RwLock<Arc<Settings>>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(Arc::new(load())))
}

/// The settings in effect. The first call reads the file; later calls only take a read lock.
pub fn current() -> Arc<Settings> {
    cache().read().unwrap_or_else(|p| p.into_inner()).clone()
}

/// Read the file again (after another process saved it). Returns what's now in effect.
pub fn reload() -> Arc<Settings> {
    let s = Arc::new(load());
    *cache().write().unwrap_or_else(|p| p.into_inner()) = s.clone();
    s
}

/// Write the settings atomically (temp file in the same folder, then rename) and make
/// them current in this process.
pub fn save(s: &Settings) -> std::io::Result<()> {
    let p = path().ok_or_else(|| std::io::Error::other("no data folder"))?;
    let dir = p
        .parent()
        .ok_or_else(|| std::io::Error::other("no data folder"))?;
    std::fs::create_dir_all(dir)?;
    let mut s = s.clone();
    s.version = VERSION;
    let json = serde_json::to_vec_pretty(&s).map_err(std::io::Error::other)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut tmp, &json)?;
    // Replacing is right: this is our own state file, never a user file.
    tmp.persist(&p).map_err(|e| e.error)?;
    *cache().write().unwrap_or_else(|p| p.into_inner()) = Arc::new(s);
    Ok(())
}

/// Forget the recent-format history the wheel ranks targets by.
pub fn clear_history() -> std::io::Result<()> {
    match crate::data_dir().map(|d| d.join("frecency.json")) {
        Some(p) if p.exists() => std::fs::remove_file(p),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerant_reading() {
        // Unknown fields are ignored, missing ones take defaults, junk gives defaults.
        let s = Settings::from_json(br#"{"theme":"dark","jobs":99,"future_thing":[1,2]}"#);
        assert_eq!(s.theme, ThemeChoice::Dark);
        assert_eq!(s.jobs, MAX_JOBS);
        assert!(s.offline && s.use_office && s.start_with_windows);
        assert_eq!(Settings::from_json(b"not json"), Settings::default());
        let round = serde_json::to_vec(&s).unwrap();
        assert_eq!(Settings::from_json(&round), s);
        assert!(Settings::default().jobs() >= 1);
        let d = Settings {
            use_ffmpeg: false,
            ..Settings::default()
        }
        .discovery();
        assert!(d.office && d.libreoffice && !d.ffmpeg);
    }
}
