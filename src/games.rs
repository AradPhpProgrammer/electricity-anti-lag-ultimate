//! Observation-only game process detection.
//!
//! Hermes never opens a game process, injects code, or changes its priority.

use std::path::Path;

#[cfg(not(target_os = "linux"))]
use std::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GameContext {
    pub name: &'static str,
    pub process: &'static str,
}

const KNOWN_GAMES: &[GameContext] = &[
    GameContext {
        name: "Counter-Strike 2",
        process: "cs2.exe",
    },
    GameContext {
        name: "VALORANT",
        process: "valorant-win64-shipping.exe",
    },
    GameContext {
        name: "Apex Legends",
        process: "r5apex.exe",
    },
    GameContext {
        name: "Fortnite",
        process: "fortniteclient-win64-shipping.exe",
    },
    GameContext {
        name: "Overwatch 2",
        process: "overwatch.exe",
    },
    GameContext {
        name: "Rocket League",
        process: "rocketleague.exe",
    },
];

pub fn detect_running_game() -> Option<GameContext> {
    detect_from_process_names(list_process_names())
}

pub fn detect_from_process_names<I, S>(names: I) -> Option<GameContext>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let normalized: Vec<String> = names
        .into_iter()
        .map(|name| normalize_process_name(name.as_ref()))
        .collect();

    KNOWN_GAMES.iter().copied().find(|game| {
        let windows_name = game.process;
        let portable_name = windows_name.trim_end_matches(".exe");
        normalized
            .iter()
            .any(|name| name == windows_name || name == portable_name)
    })
}

fn normalize_process_name(value: &str) -> String {
    Path::new(value.trim())
        .file_name()
        .and_then(|part| part.to_str())
        .unwrap_or(value.trim())
        .to_ascii_lowercase()
}

#[cfg(target_os = "linux")]
fn list_process_names() -> Vec<String> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|byte| byte.is_ascii_digit())
        })
        .filter_map(|entry| std::fs::read_to_string(entry.path().join("comm")).ok())
        .map(|name| name.trim().to_owned())
        .collect()
}

#[cfg(target_os = "windows")]
fn list_process_names() -> Vec<String> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = Command::new("tasklist")
        .args(["/fo", "csv", "/nh"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    output
        .ok()
        .map(|result| String::from_utf8_lossy(&result.stdout).into_owned())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.split(',').next())
        .map(|name| name.trim_matches('"').to_owned())
        .collect()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn list_process_names() -> Vec<String> {
    let output = Command::new("ps").args(["-axo", "comm="]).output();
    output
        .ok()
        .map(|result| String::from_utf8_lossy(&result.stdout).into_owned())
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cs2_on_windows_or_linux() {
        assert_eq!(
            detect_from_process_names(["steam.exe", "CS2.EXE"]).map(|game| game.name),
            Some("Counter-Strike 2")
        );
        assert_eq!(
            detect_from_process_names(["/games/cs2"]).map(|game| game.name),
            Some("Counter-Strike 2")
        );
    }

    #[test]
    fn unknown_processes_are_ignored() {
        assert!(detect_from_process_names(["explorer.exe", "bash"]).is_none());
    }
}
