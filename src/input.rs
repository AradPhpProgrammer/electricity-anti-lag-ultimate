//! Input subsystem: raw-input Hz detection, floating-mouse filter,
//! and click interrupt (Layer 3 of the PLL engine).
//!
//! Design goals:
//!   * Zero blocking I/O on the worker thread — all state is atomic.
//!   * On non-Windows platforms, the modules expose no-op stubs so the
//!     GUI and benchmarks can compile and run for development.
//!   * Hz detector uses an EMA + snap-to-candidate to ignore jittery
//!     short-term readings.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Polling rate mode selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HzMode {
    Auto,
    Manual,
}

/// Immutable polling-rate configuration transferred from the UI to the
/// worker. The detector itself remains worker-local.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HzConfig {
    pub mode: HzMode,
    pub manual_hz: u32,
}

const MANUAL_MODE_BIT: u32 = 1 << 31;
const HZ_VALUE_MASK: u32 = !MANUAL_MODE_BIT;

impl HzConfig {
    fn pack(self) -> u32 {
        let mode = match self.mode {
            HzMode::Auto => 0,
            HzMode::Manual => MANUAL_MODE_BIT,
        };
        mode | self.manual_hz.min(HZ_VALUE_MASK)
    }

    fn unpack(packed: u32) -> Self {
        Self {
            mode: if packed & MANUAL_MODE_BIT == 0 {
                HzMode::Auto
            } else {
                HzMode::Manual
            },
            manual_hz: packed & HZ_VALUE_MASK,
        }
    }
}

/// Packed atomic UI-to-worker polling-rate control. Keeping the mode and
/// manual rate in one word gives each worker tick one coherent config load.
pub struct HzControl {
    packed: AtomicU32,
}

impl HzControl {
    pub fn new(mode: HzMode, manual_hz: u32) -> Self {
        Self {
            packed: AtomicU32::new(HzConfig { mode, manual_hz }.pack()),
        }
    }

    pub fn store(&self, mode: HzMode, manual_hz: u32) {
        self.packed
            .store(HzConfig { mode, manual_hz }.pack(), Ordering::Release);
    }

    pub fn load(&self) -> HzConfig {
        HzConfig::unpack(self.packed.load(Ordering::Acquire))
    }
}

impl HzMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Manual => "Manual",
        }
    }
}

/// Known polling-rate candidates (Hz). Values outside this set will be
/// coerced to the nearest candidate when in Auto mode.
pub const POLLING_CANDIDATES: &[u32] = &[125, 250, 500, 1000, 2000, 4000, 8000];

/// Snap a raw Hz measurement to the nearest known polling rate.
/// Capped to 1000 Hz to prevent UI misreads of sub-rate noise.
pub fn snap_polling_rate(raw_hz: f64) -> u32 {
    let mut best = 125u32;
    let mut best_diff = f64::MAX;
    for &c in POLLING_CANDIDATES {
        let d = (raw_hz - c as f64).abs();
        if d < best_diff {
            best_diff = d;
            best = c;
        }
    }
    best.min(1000).max(125)
}

/// Auto/Manual Hz detector owned exclusively by the timing worker.
pub struct HzDetector {
    pub ema_hz: f64,
    pub samples: u32,
    pub mode: HzMode,
    pub manual_hz: u32,
    pub last_us: f64,
}

impl HzDetector {
    pub fn new(mode: HzMode, manual_hz: u32) -> Self {
        Self {
            ema_hz: 125.0,
            samples: 0,
            mode,
            manual_hz,
            last_us: 0.0,
        }
    }

    pub fn feed_us(&mut self, now_us: f64) {
        if self.last_us > 0.0 {
            let dt = (now_us - self.last_us).max(1.0);
            if dt > 100.0 && dt < 20_000.0 {
                let hz = 1_000_000.0 / dt;
                self.ema_hz = self.ema_hz * 0.7 + hz * 0.3;
                self.samples = self.samples.saturating_add(1);
                if self.ema_hz > 1000.0 {
                    self.ema_hz = 1000.0;
                }
            }
        }
        self.last_us = now_us;
    }

    pub fn resolved_hz(&self) -> u32 {
        match self.mode {
            HzMode::Manual => self.manual_hz,
            HzMode::Auto => snap_polling_rate(self.ema_hz),
        }
    }

    pub fn set_manual(&mut self, hz: u32) {
        self.mode = HzMode::Manual;
        self.manual_hz = hz;
    }

    pub fn set_auto(&mut self) {
        self.mode = HzMode::Auto;
    }

    pub fn apply_config(&mut self, config: HzConfig) {
        self.manual_hz = config.manual_hz;
        self.mode = config.mode;
    }
}

/// Floating-mouse filter. The "floating" symptom (small phantom
/// movement, sluggish feel) is caused by uneven USB polling, so we
/// classify each delta as a spike vs. normal motion and emit a smoothed
/// value with a gentle low-pass filter.
#[derive(Clone, Copy, Debug)]
pub struct FloatingMouseFilter {
    pub last_delta: f64,
    pub last_out: f64,
    pub alpha: f64,
    pub spike_threshold: f64,
}

impl Default for FloatingMouseFilter {
    fn default() -> Self {
        Self {
            last_delta: 0.0,
            last_out: 0.0,
            alpha: 0.6,
            spike_threshold: 60.0,
        }
    }
}

impl FloatingMouseFilter {
    /// Feed a raw mouse delta (in arbitrary units — pixels per poll) and
    /// receive a smoothed delta. Outliers are clamped rather than
    /// dropped, which avoids the "sticky release" artefact.
    pub fn filter(&mut self, raw: f64) -> f64 {
        let smoothed = self.alpha * self.last_out + (1.0 - self.alpha) * raw;
        let delta_from_prev = (raw - self.last_delta).abs();
        let out = if delta_from_prev > self.spike_threshold {
            // Spike — clamp toward the smoothed value, don't propagate.
            self.last_out + (smoothed - self.last_out) * 0.1
        } else {
            smoothed
        };
        self.last_delta = raw;
        self.last_out = out;
        out
    }

    pub fn reset(&mut self) {
        self.last_delta = 0.0;
        self.last_out = 0.0;
    }
}

/// Click interrupt (Layer 3). Atomic flag; the worker consumes it on
/// each tick boundary for instant phase resync.
#[derive(Debug)]
pub struct ClickInterrupt {
    flag: AtomicBool,
}

impl Default for ClickInterrupt {
    fn default() -> Self {
        Self::new()
    }
}

impl ClickInterrupt {
    pub fn new() -> Self {
        Self {
            flag: AtomicBool::new(false),
        }
    }

    pub fn trigger(&self) {
        self.flag.store(true, Ordering::Release);
    }

    pub fn consume(&self) -> bool {
        self.flag.swap(false, Ordering::AcqRel)
    }

    pub fn is_pending(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

/// Mouse Delta Modulator (Layer 2). Maintains a velocity EMA and a
/// target phase shift, exposed to the PLL worker thread.
#[derive(Debug)]
pub struct MouseModulator {
    pub velocity_ema: f64,
    pub last_event_us: f64,
    pub phase_shift_us: f64,
}

impl Default for MouseModulator {
    fn default() -> Self {
        Self::new()
    }
}

impl MouseModulator {
    pub fn new() -> Self {
        Self {
            velocity_ema: 0.0,
            last_event_us: 0.0,
            phase_shift_us: 0.0,
        }
    }

    /// Feed a mouse-event timestamp. Returns the suggested phase shift
    /// in microseconds (always positive — phase shift only moves earlier).
    pub fn feed(&mut self, now_us: f64, remaining_us: f64) -> f64 {
        if self.last_event_us > 0.0 {
            let dt = (now_us - self.last_event_us).max(1.0);
            let v = 1000.0 / dt;
            self.velocity_ema = 0.8 * self.velocity_ema + 0.2 * v;
        }
        self.last_event_us = now_us;
        let nv = (self.velocity_ema / 2000.0).min(1.0);
        self.phase_shift_us = nv * remaining_us * 0.25;
        self.phase_shift_us
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_polling_picks_nearest() {
        assert_eq!(snap_polling_rate(990.0), 1000);
        assert_eq!(snap_polling_rate(620.0), 500);
        assert_eq!(snap_polling_rate(130.0), 125);
    }

    #[test]
    fn hz_control_packs_mode_and_manual_rate_coherently() {
        let control = HzControl::new(HzMode::Auto, 1000);
        assert_eq!(
            control.load(),
            HzConfig {
                mode: HzMode::Auto,
                manual_hz: 1000,
            }
        );

        control.store(HzMode::Manual, 8000);
        assert_eq!(
            control.load(),
            HzConfig {
                mode: HzMode::Manual,
                manual_hz: 8000,
            }
        );
    }

    #[test]
    fn click_interrupt_roundtrip() {
        let ci = ClickInterrupt::new();
        assert!(!ci.is_pending());
        ci.trigger();
        assert!(ci.is_pending());
        assert!(ci.consume());
        assert!(!ci.consume());
    }

    #[test]
    fn floating_filter_smooths_constant_input() {
        let mut f = FloatingMouseFilter::default();
        let mut last = 0.0;
        for _ in 0..20 {
            last = f.filter(2.0);
        }
        // After 20 iterations of constant 2.0 the filter must converge.
        assert!((last - 2.0).abs() < 0.5, "got {last}");
    }

    #[test]
    fn floating_filter_clamps_spikes() {
        let mut f = FloatingMouseFilter::default();
        for _ in 0..10 {
            let _ = f.filter(2.0);
        }
        // Inject a spike — output must NOT jump to the spike value.
        let out = f.filter(200.0);
        assert!(out < 100.0, "spike leaked through: {out}");
    }
}
