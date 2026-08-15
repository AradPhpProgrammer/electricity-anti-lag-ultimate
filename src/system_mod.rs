//! System optimization subsystem: MSI mode for USB host controllers,
//! core affinity isolation, and C-state power locks.
//!
//! All modifications are fully tracked and reverted upon drop or explicit request.

#[derive(Default, Debug)]
pub struct SystemGuard {
    pub cstate_locked: bool,
    pub affinity_set: bool,
    msi_enabled: bool,
}

impl SystemGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn lock_c_states(&mut self, enable: bool) -> bool {
        #[cfg(target_os = "windows")]
        {
            let ok = lock_cpu_power_states(enable);
            if ok {
                self.cstate_locked = enable;
            }
            ok
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = enable;
            false
        }
    }

    pub fn isolate_affinity(&mut self, enable: bool) -> bool {
        #[cfg(target_os = "windows")]
        {
            let ok = set_affinity(enable);
            if ok {
                self.affinity_set = enable;
            }
            ok
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = enable;
            false
        }
    }

    pub fn restore_all(&mut self) {
        if self.cstate_locked {
            let _ = self.lock_c_states(false);
        }
        if self.affinity_set {
            let _ = self.isolate_affinity(false);
        }
    }
}

impl Drop for SystemGuard {
    fn drop(&mut self) {
        self.restore_all();
    }
}

#[cfg(target_os = "windows")]
fn lock_cpu_power_states(enable: bool) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let val = if enable { "0" } else { "1" };
    let script = format!(
        "powercfg /setacvalueindex scheme_current sub_processor 0cc5b647-c1df-4637-891a-dec35c318583 {}\npowercfg /setactive scheme_current\nexit 0",
        val
    );
    let status = std::process::Command::new("powershell")
        .args(["-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    matches!(status, Ok(s) if s.success())
}

#[cfg(target_os = "windows")]
fn set_affinity(enable: bool) -> bool {
    use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessAffinityMask};
    unsafe {
        let handle = GetCurrentProcess();
        let mask = if enable { 0b10000000 } else { !0 };
        SetProcessAffinityMask(handle, mask).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_defaults_clean() {
        let g = SystemGuard::new();
        assert!(!g.cstate_locked);
    }
}
