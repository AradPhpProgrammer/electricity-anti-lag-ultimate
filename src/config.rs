//! Simple, dependency-free configuration file handling with automatic
//! backup on save and a portable-mode override.

use std::fs;
use std::path::{Path, PathBuf};

use crate::input::HzMode;
use crate::pll::{PllConfig, TickRate};

#[derive(Clone, Debug)]
pub struct HermesConfig {
    pub tick_rate: TickRate,
    pub pll: PllConfig,
    pub hz_mode: HzMode,
    pub manual_hz: u32,
    pub timer_resolution: bool,
    pub ram_only: bool,
    pub path: PathBuf,
}

impl Default for HermesConfig {
    fn default() -> Self {
        Self {
            tick_rate: TickRate::Hz128,
            pll: PllConfig::default(),
            hz_mode: HzMode::Auto,
            manual_hz: 1000,
            timer_resolution: true,
            ram_only: false,
            path: Self::default_path(),
        }
    }
}

impl HermesConfig {
    /// %APPDATA%\HermesEngine\config.toml on Windows, ~/.config/hermes-engine/config.toml elsewhere.
    pub fn default_path() -> PathBuf {
        #[cfg(target_os = "windows")]
        {
            if let Ok(appdata) = std::env::var("APPDATA") {
                return PathBuf::from(appdata)
                    .join("HermesEngine")
                    .join("config.toml");
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            if let Ok(home) = std::env::var("HOME") {
                return PathBuf::from(home)
                    .join(".config")
                    .join("hermes-engine")
                    .join("config.toml");
            }
        }
        PathBuf::from("config.toml")
    }

    pub fn portable_path() -> Option<PathBuf> {
        std::env::current_exe().ok().and_then(|exe| {
            let dir = exe.parent()?;
            let candidate = dir.join("config.toml");
            if candidate.exists() {
                Some(candidate)
            } else {
                None
            }
        })
    }

    pub fn load() -> Self {
        let path = Self::portable_path().unwrap_or_else(Self::default_path);
        let mut cfg = Self::default();
        cfg.path = path.clone();
        if let Ok(content) = fs::read_to_string(&path) {
            cfg.parse(&content);
        }
        cfg
    }

    fn parse(&mut self, content: &str) {
        for raw in content.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            match key {
                "tick_rate" => {
                    if let Ok(hz) = value.parse::<u32>() {
                        self.tick_rate = match hz {
                            64 => TickRate::Hz64,
                            _ => TickRate::Hz128,
                        };
                    }
                }
                "pll_kp" => {
                    if let Ok(v) = value.parse::<f64>() {
                        self.pll.kp = v;
                    }
                }
                "pll_ki" => {
                    if let Ok(v) = value.parse::<f64>() {
                        self.pll.ki = v;
                    }
                }
                "pll_lfo_amp" => {
                    if let Ok(v) = value.parse::<f64>() {
                        self.pll.lfo_amp_us = v;
                    }
                }
                "pll_lfo_period" => {
                    if let Ok(v) = value.parse::<f64>() {
                        self.pll.lfo_period_s = v;
                    }
                }
                "hz_mode" => {
                    if value.eq_ignore_ascii_case("manual") {
                        self.hz_mode = HzMode::Manual;
                    } else {
                        self.hz_mode = HzMode::Auto;
                    }
                }
                "manual_hz" => {
                    if let Ok(v) = value.parse::<u32>() {
                        self.manual_hz = v;
                    }
                }
                "timer_resolution" => {
                    if let Ok(v) = value.parse::<bool>() {
                        self.timer_resolution = v;
                    }
                }
                "ram_only" => {
                    if let Ok(v) = value.parse::<bool>() {
                        self.ram_only = v;
                    }
                }
                _ => {}
            }
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Backup existing file once per save.
        if self.path.exists() {
            let backup = self.path.with_extension("toml.bak");
            let _ = fs::copy(&self.path, backup);
        }
        let mut content = String::new();
        content.push_str("# Hermes Engine Ultimate config\n");
        content.push_str(&format!("tick_rate={}\n", self.tick_rate.hz()));
        content.push_str(&format!("pll_kp={}\n", self.pll.kp));
        content.push_str(&format!("pll_ki={}\n", self.pll.ki));
        content.push_str(&format!("pll_lfo_amp={}\n", self.pll.lfo_amp_us));
        content.push_str(&format!("pll_lfo_period={}\n", self.pll.lfo_period_s));
        content.push_str(&format!(
            "hz_mode={}\n",
            match self.hz_mode {
                HzMode::Auto => "auto",
                HzMode::Manual => "manual",
            }
        ));
        content.push_str(&format!("manual_hz={}\n", self.manual_hz));
        content.push_str(&format!("timer_resolution={}\n", self.timer_resolution));
        content.push_str(&format!("ram_only={}\n", self.ram_only));
        fs::write(&self.path, content)
    }

    pub fn save_to(&mut self, path: &Path) -> std::io::Result<()> {
        self.path = path.to_path_buf();
        self.save()
    }
}
