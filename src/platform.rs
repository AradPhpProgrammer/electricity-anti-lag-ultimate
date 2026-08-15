//! Cross-platform utilities: timer resolution guard, high-resolution clock,
//! and platform identifiers.

use std::time::{Duration, Instant};

/// Windows 1 ms timer resolution guard. On non-Windows it's a no-op.
/// Always paired with `timeEndPeriod` on drop.
#[derive(Debug)]
pub struct TimerResolutionGuard {
    active: bool,
}

impl TimerResolutionGuard {
    pub fn request(enabled: bool) -> Self {
        if !enabled {
            return Self { active: false };
        }
        #[cfg(target_os = "windows")]
        {
            use windows::Win32::Media::timeBeginPeriod;
            // TIMERR_NOERROR == 0. We never assume success.
            let active = unsafe { timeBeginPeriod(1) } == 0;
            Self { active }
        }
        #[cfg(not(target_os = "windows"))]
        {
            Self { active: false }
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }
}

impl Drop for TimerResolutionGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        if self.active {
            use windows::Win32::Media::timeEndPeriod;
            unsafe {
                let _ = timeEndPeriod(1);
            }
        }
    }
}

pub fn platform_name() -> &'static str {
    std::env::consts::OS
}

pub fn timer_request_scope() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "Windows process-scoped 1 ms request"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "Not used on this platform"
    }
}

/// A high-resolution monotonic clock built on `Instant`. Used by the PLL
/// engine to compute elapsed microseconds with sub-microsecond precision.
#[derive(Clone, Copy)]
pub struct HighResClock {
    origin: Instant,
}

impl HighResClock {
    pub fn new() -> Self {
        Self { origin: Instant::now() }
    }

    pub fn now_us(&self) -> f64 {
        self.origin.elapsed().as_secs_f64() * 1_000_000.0
    }

    pub fn now_ns(&self) -> u128 {
        self.origin.elapsed().as_nanos()
    }
}

impl Default for HighResClock {
    fn default() -> Self {
        Self::new()
    }
}

/// A monotonic-clock period estimator. Uses a ring buffer of inter-sample
/// intervals to compute mean, std-dev, and a coefficient-of-variation
/// (CV) that detects clock drift — the kind that causes "floating mouse"
/// symptoms on a noisy USB bus or unstable VRM power.
pub struct ClockStabilityMonitor {
    intervals: Vec<f64>,
    pos: usize,
    last_us: Option<f64>,
}

impl ClockStabilityMonitor {
    pub fn new(capacity: u32) -> Self {
        let cap = capacity.max(8) as usize;
        Self {
            intervals: Vec::with_capacity(cap),
            pos: 0,
            last_us: None,
        }
    }

    pub fn observe(&mut self, now_us: f64) {
        if let Some(prev) = self.last_us {
            let dt = (now_us - prev).max(0.0);
            if self.intervals.len() < self.intervals.capacity() {
                self.intervals.push(dt);
            } else {
                if self.pos >= self.intervals.len() {
                    self.pos = 0;
                }
                self.intervals[self.pos] = dt;
                self.pos += 1;
            }
        }
        self.last_us = Some(now_us);
    }

    pub fn mean_us(&self) -> f64 {
        if self.intervals.is_empty() {
            0.0
        } else {
            self.intervals.iter().sum::<f64>() / self.intervals.len() as f64
        }
    }

    pub fn std_dev_us(&self) -> f64 {
        if self.intervals.len() < 2 {
            return 0.0;
        }
        let mean = self.mean_us();
        let var: f64 = self
            .intervals
            .iter()
            .map(|v| (v - mean).powi(2))
            .sum::<f64>()
            / self.intervals.len() as f64;
        var.sqrt()
    }

    /// Coefficient of variation (std_dev / mean). A well-behaved clock
    /// should have CV < 0.05 (5%). Values above 0.20 indicate significant
    /// jitter. This replaces the old max/mean ratio which was wildly
    /// inaccurate and reported absurd numbers like 950.
    pub fn instability_ratio(&self) -> f64 {
        let m = self.mean_us();
        if m < f64::EPSILON {
            0.0
        } else {
            self.std_dev_us() / m
        }
    }

    pub fn samples(&self) -> u32 {
        self.intervals.len() as u32
    }
}

impl Default for ClockStabilityMonitor {
    fn default() -> Self {
        Self::new(256)
    }
}

/// Thread identity helper. Returns a stable thread name string.
pub fn current_thread_label() -> &'static str {
    "hermes-main"
}

/// Convert a duration to a µs float with safe rounding.
pub fn duration_to_us(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_timer_request_is_inactive() {
        assert!(!TimerResolutionGuard::request(false).is_active());
    }

    #[test]
    fn clock_stability_accumulates() {
        let mut mon = ClockStabilityMonitor::new(16);
        let mut t = 0.0_f64;
        for _ in 0..17 {
            mon.observe(t);
            t += 1000.0;
        }
        assert_eq!(mon.samples(), 16);
        assert!((mon.mean_us() - 1000.0).abs() < 0.001);
        // Constant intervals → std-dev = 0 → CV = 0.
        assert!(mon.instability_ratio() < 0.001);
    }

    #[test]
    fn clock_instability_detects_jitter() {
        let mut mon = ClockStabilityMonitor::new(64);
        let mut t = 0.0_f64;
        // Intervals alternate 900 / 1100 → mean 1000, std-dev ~100 → CV ~0.10.
        for i in 0..100 {
            t += if i % 2 == 0 { 900.0 } else { 1100.0 };
            mon.observe(t);
        }
        let cv = mon.instability_ratio();
        assert!(cv > 0.05 && cv < 0.15, "expected CV ~0.10, got {cv}");
    }
}
