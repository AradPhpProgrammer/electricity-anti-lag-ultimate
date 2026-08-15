//! Windows network stack optimizations (reversible & safe).
//!
//! Applies TCP Nagle/ACK frequency registry options temporarily when requested,
//! and automatically reverts them when `NetworkGuard` is dropped or disabled.
//!
//! On non-Windows platforms, functions return safe no-op results.

#[derive(Debug, Default)]
pub struct NetworkGuard {
    applied: bool,
}

impl NetworkGuard {
    pub fn new() -> Self {
        Self { applied: false }
    }

    pub fn apply(&mut self) -> bool {
        #[cfg(target_os = "windows")]
        {
            let ok = apply_network_stack(true);
            self.applied = ok;
            ok
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    pub fn revert(&mut self) -> bool {
        if !self.applied {
            return true;
        }
        #[cfg(target_os = "windows")]
        {
            let ok = apply_network_stack(false);
            self.applied = !ok;
            ok
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.applied = false;
            true
        }
    }

    pub fn is_applied(&self) -> bool {
        self.applied
    }
}

impl Drop for NetworkGuard {
    fn drop(&mut self) {
        let _ = self.revert();
    }
}

#[cfg(target_os = "windows")]
fn apply_network_stack(enable: bool) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let (ack, nodelay, delack) = if enable {
        ("1", "1", "0")
    } else {
        ("2", "0", "2")
    };

    let script = format!(
        r#"$interfaces = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\*'
        foreach ($int in $interfaces) {{
            Set-ItemProperty -Path $int.PSPath -Name 'TcpAckFrequency' -Value {} -Type DWord -Force
            Set-ItemProperty -Path $int.PSPath -Name 'TCPNoDelay' -Value {} -Type DWord -Force
            Set-ItemProperty -Path $int.PSPath -Name 'TcpDelAckTicks' -Value {} -Type DWord -Force
        }}
        exit 0"#,
        ack, nodelay, delack
    );

    let status = std::process::Command::new("powershell")
        .args([
            "-NonInteractive",
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &script,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .status();

    matches!(status, Ok(s) if s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_defaults_to_unapplied() {
        let g = NetworkGuard::new();
        assert!(!g.is_applied());
    }
}
