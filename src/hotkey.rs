//! High-performance async hotkey poller via `GetAsyncKeyState`.
//!
//! Works in 100% of full-screen games (DirectX, Vulkan, OpenGL, Borderless)
//! without needing Windows message pumps or invisible windows.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_MENU};

/// Global hotkey event flags.
pub struct HotkeyState {
    pub engine_toggle_pending: AtomicBool,
    pub click_shot_pending: AtomicBool,
    pub reset_pending: AtomicBool,
}

impl HotkeyState {
    pub fn new() -> Self {
        Self {
            engine_toggle_pending: AtomicBool::new(false),
            click_shot_pending: AtomicBool::new(false),
            reset_pending: AtomicBool::new(false),
        }
    }
}

impl Default for HotkeyState {
    fn default() -> Self {
        Self::new()
    }
}

static TOGGLE_FLAG: AtomicBool = AtomicBool::new(false);
static CLICK_FLAG: AtomicBool = AtomicBool::new(false);
static RESET_FLAG: AtomicBool = AtomicBool::new(false);

pub fn drain_toggle() -> bool {
    TOGGLE_FLAG.swap(false, Ordering::AcqRel)
}
pub fn drain_click() -> bool {
    CLICK_FLAG.swap(false, Ordering::AcqRel)
}
pub fn drain_reset() -> bool {
    RESET_FLAG.swap(false, Ordering::AcqRel)
}

/// Spawns a dedicated low-overhead thread polling Alt+1, Alt+2, Alt+3 every 30 ms.
#[cfg(target_os = "windows")]
pub fn spawn_hotkey_thread() -> Arc<HotkeyState> {
    let state = Arc::new(HotkeyState::new());

    thread::Builder::new()
        .name("hermes-async-hotkey".into())
        .spawn(move || {
            let mut alt1_pressed = false;
            let mut alt2_pressed = false;
            let mut alt3_pressed = false;

            loop {
                thread::sleep(Duration::from_millis(30));

                unsafe {
                    // Check if ALT key is held down
                    let alt_down = (GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0;

                    if alt_down {
                        // Key '1' (0x31)
                        let key1_down = (GetAsyncKeyState(0x31) as u16 & 0x8000) != 0;
                        if key1_down && !alt1_pressed {
                            TOGGLE_FLAG.store(true, Ordering::Release);
                            alt1_pressed = true;
                        } else if !key1_down {
                            alt1_pressed = false;
                        }

                        // Key '2' (0x32)
                        let key2_down = (GetAsyncKeyState(0x32) as u16 & 0x8000) != 0;
                        if key2_down && !alt2_pressed {
                            CLICK_FLAG.store(true, Ordering::Release);
                            alt2_pressed = true;
                        } else if !key2_down {
                            alt2_pressed = false;
                        }

                        // Key '3' (0x33)
                        let key3_down = (GetAsyncKeyState(0x33) as u16 & 0x8000) != 0;
                        if key3_down && !alt3_pressed {
                            RESET_FLAG.store(true, Ordering::Release);
                            alt3_pressed = true;
                        } else if !key3_down {
                            alt3_pressed = false;
                        }
                    } else {
                        alt1_pressed = false;
                        alt2_pressed = false;
                        alt3_pressed = false;
                    }
                }
            }
        })
        .expect("failed to spawn hotkey thread");

    state
}

#[cfg(not(target_os = "windows"))]
pub fn spawn_hotkey_thread() -> Arc<HotkeyState> {
    Arc::new(HotkeyState::new())
}
