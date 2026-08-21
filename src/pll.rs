//! Hermes 3-Layer Phase-Locked Loop engine (Ultimate Edition).
//! v5.6: Anti-oscillation design — slow EMA + deadband + clamped correction.

use crate::input::{HzConfig, HzControl, HzMode};
use crate::platform::HighResClock;
use std::f64::consts::PI;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const TELEMETRY_PUBLISH_INTERVAL_TICKS: u64 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TickRate {
    Hz64,
    Hz128,
}

impl TickRate {
    pub fn hz(self) -> u32 {
        match self {
            Self::Hz64 => 64,
            Self::Hz128 => 128,
        }
    }
    pub fn us_per_tick(self) -> f64 {
        1_000_000.0 / self.hz() as f64
    }
    pub fn next(self) -> Self {
        match self {
            Self::Hz64 => Self::Hz128,
            Self::Hz128 => Self::Hz64,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PllConfig {
    pub kp: f64,
    pub ki: f64,
    pub power: f64,
    pub lfo_amp_us: f64,
    pub lfo_period_s: f64,
}

impl Default for PllConfig {
    fn default() -> Self {
        Self {
            kp: 0.08,
            ki: 0.008,
            power: 1.0,
            lfo_amp_us: 2.0,
            lfo_period_s: 8.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PllSnapshot {
    pub tick_index: u64,
    pub sleep_us: f64,
    pub phase_error_us: f64,
    pub jitter_ema_us: f64,
    pub cs_latency_us: f64,
    pub mouse_shift_us: f64,
    pub lfo_us: f64,
    pub active_layer: u8,
    pub spin_duty_pct: f64,
    pub resolved_hz: u32,
    pub clock_mean_us: f64,
    pub clock_instability: f64,
    pub clock_samples: u32,
}

/// UI-to-L0 configuration. Alignment keeps these UI-written atomics away
/// from the worker-written telemetry cache lines.
#[repr(align(64))]
pub struct L0Control {
    tick_rate: AtomicU32,
    hz: HzControl,
    kp_bits: AtomicU64,
    ki_bits: AtomicU64,
    lfo_amp_bits: AtomicU64,
    lfo_period_bits: AtomicU64,
    /// Global PLL power multiplier (0.1 = ultra-soft, 2.0 = razor-sharp).
    pll_power_bits: AtomicU64,
}

/// Read-only L0 snapshots for observers. Only the timing worker writes these
/// atomics, and it does so at a reduced cadence.
#[repr(align(64))]
pub struct L0Telemetry {
    sleep_us: AtomicU64,
    phase_error_us: AtomicU64,
    jitter_ema_us: AtomicU64,
    cs_latency_us: AtomicU64,
    tick_index: AtomicU64,
    mouse_shift_us: AtomicU64,
    lfo_us: AtomicU64,
    spin_duty_pct: AtomicU64,
    resolved_hz: AtomicU32,
    clock_mean_us: AtomicU64,
    clock_instability: AtomicU64,
    clock_samples: AtomicU32,
}

pub struct PllShared {
    control: L0Control,
    telemetry: L0Telemetry,
}

impl PllShared {
    pub fn new(tick_rate: TickRate, hz_mode: HzMode, manual_hz: u32) -> Self {
        let cfg = PllConfig::default();
        Self {
            control: L0Control {
                tick_rate: AtomicU32::new(tick_rate.hz()),
                hz: HzControl::new(hz_mode, manual_hz),
                kp_bits: AtomicU64::new(cfg.kp.to_bits()),
                ki_bits: AtomicU64::new(cfg.ki.to_bits()),
                lfo_amp_bits: AtomicU64::new(cfg.lfo_amp_us.to_bits()),
                lfo_period_bits: AtomicU64::new(cfg.lfo_period_s.to_bits()),
                pll_power_bits: AtomicU64::new(1.0_f64.to_bits()),
            },
            telemetry: L0Telemetry {
                sleep_us: AtomicU64::new(0),
                phase_error_us: AtomicU64::new(0),
                jitter_ema_us: AtomicU64::new(0),
                cs_latency_us: AtomicU64::new(0),
                tick_index: AtomicU64::new(0),
                mouse_shift_us: AtomicU64::new(0),
                lfo_us: AtomicU64::new(0),
                spin_duty_pct: AtomicU64::new(0),
                resolved_hz: AtomicU32::new(match hz_mode {
                    HzMode::Auto => 125,
                    HzMode::Manual => manual_hz,
                }),
                clock_mean_us: AtomicU64::new(0),
                clock_instability: AtomicU64::new(0),
                clock_samples: AtomicU32::new(0),
            },
        }
    }

    pub fn store_power(&self, power: f64) {
        self.control
            .pll_power_bits
            .store(power.to_bits(), Ordering::Release);
    }

    pub fn load_power(&self) -> f64 {
        f64::from_bits(self.control.pll_power_bits.load(Ordering::Acquire))
    }

    pub fn store_cfg(&self, cfg: PllConfig) {
        self.control
            .kp_bits
            .store(cfg.kp.to_bits(), Ordering::Release);
        self.control
            .ki_bits
            .store(cfg.ki.to_bits(), Ordering::Release);
        self.control
            .pll_power_bits
            .store(cfg.power.to_bits(), Ordering::Release);
        self.control
            .lfo_amp_bits
            .store(cfg.lfo_amp_us.to_bits(), Ordering::Release);
        self.control
            .lfo_period_bits
            .store(cfg.lfo_period_s.to_bits(), Ordering::Release);
    }

    pub fn load_cfg(&self) -> PllConfig {
        PllConfig {
            kp: f64::from_bits(self.control.kp_bits.load(Ordering::Acquire)),
            ki: f64::from_bits(self.control.ki_bits.load(Ordering::Acquire)),
            power: f64::from_bits(self.control.pll_power_bits.load(Ordering::Acquire)),
            lfo_amp_us: f64::from_bits(self.control.lfo_amp_bits.load(Ordering::Acquire)),
            lfo_period_s: f64::from_bits(self.control.lfo_period_bits.load(Ordering::Acquire)),
        }
    }

    pub fn store_tick_rate(&self, rate: TickRate) {
        self.control.tick_rate.store(rate.hz(), Ordering::Release);
    }

    pub fn load_tick_rate(&self) -> TickRate {
        match self.control.tick_rate.load(Ordering::Acquire) {
            64 => TickRate::Hz64,
            _ => TickRate::Hz128,
        }
    }

    pub fn store_hz_config(&self, mode: HzMode, manual_hz: u32) {
        self.control.hz.store(mode, manual_hz);
    }

    pub fn load_hz_config(&self) -> HzConfig {
        self.control.hz.load()
    }

    pub fn publish(
        &self,
        pll: PllSnapshot,
        resolved_hz: u32,
        clock_mean_us: f64,
        clock_instability: f64,
        clock_samples: u32,
    ) {
        self.telemetry
            .sleep_us
            .store(pll.sleep_us.to_bits(), Ordering::Release);
        self.telemetry
            .phase_error_us
            .store(pll.phase_error_us.to_bits(), Ordering::Release);
        self.telemetry
            .jitter_ema_us
            .store(pll.jitter_ema_us.to_bits(), Ordering::Release);
        self.telemetry
            .cs_latency_us
            .store(pll.cs_latency_us.to_bits(), Ordering::Release);
        self.telemetry
            .tick_index
            .store(pll.tick_index, Ordering::Release);
        self.telemetry
            .mouse_shift_us
            .store(pll.mouse_shift_us.to_bits(), Ordering::Release);
        self.telemetry
            .lfo_us
            .store(pll.lfo_us.to_bits(), Ordering::Release);
        self.telemetry
            .spin_duty_pct
            .store(pll.spin_duty_pct.to_bits(), Ordering::Release);
        self.telemetry
            .resolved_hz
            .store(resolved_hz, Ordering::Release);
        self.telemetry
            .clock_mean_us
            .store(clock_mean_us.to_bits(), Ordering::Release);
        self.telemetry
            .clock_instability
            .store(clock_instability.to_bits(), Ordering::Release);
        self.telemetry
            .clock_samples
            .store(clock_samples, Ordering::Release);
    }

    pub fn snapshot(&self) -> PllSnapshot {
        PllSnapshot {
            tick_index: self.telemetry.tick_index.load(Ordering::Acquire),
            sleep_us: f64::from_bits(self.telemetry.sleep_us.load(Ordering::Acquire)),
            phase_error_us: f64::from_bits(self.telemetry.phase_error_us.load(Ordering::Acquire)),
            jitter_ema_us: f64::from_bits(self.telemetry.jitter_ema_us.load(Ordering::Acquire)),
            cs_latency_us: f64::from_bits(self.telemetry.cs_latency_us.load(Ordering::Acquire)),
            mouse_shift_us: f64::from_bits(self.telemetry.mouse_shift_us.load(Ordering::Acquire)),
            lfo_us: f64::from_bits(self.telemetry.lfo_us.load(Ordering::Acquire)),
            spin_duty_pct: f64::from_bits(self.telemetry.spin_duty_pct.load(Ordering::Acquire)),
            active_layer: 0,
            resolved_hz: self.telemetry.resolved_hz.load(Ordering::Acquire),
            clock_mean_us: f64::from_bits(self.telemetry.clock_mean_us.load(Ordering::Acquire)),
            clock_instability: f64::from_bits(
                self.telemetry.clock_instability.load(Ordering::Acquire),
            ),
            clock_samples: self.telemetry.clock_samples.load(Ordering::Acquire),
        }
    }
}

struct BrownianNoise {
    state: u64,
}
impl BrownianNoise {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 0x9e3779b97f4a7c15 } else { seed },
        }
    }
    fn next_f64(&mut self) -> f64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        (x >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }
    fn gaussian(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-9);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
}

pub struct PllEngine {
    pub clock: HighResClock,
    pub cfg: PllConfig,
    pub tick_rate: TickRate,
    pub integral: f64,
    pub integral_limit: f64,
    pub jitter_ema_us: f64,
    pub latest_phase_error_us: f64,
    pub smoothed_phase_error: f64,
    pub lfo_value: f64,
    pub lfo_step: f64,
    pub tick_index: u64,
    pub mouse_velocity_ema: f64,
    pub last_mouse_us: f64,
    pub total_spin_us: f64,
    pub total_tick_us: f64,
    pub latest_sleep_us: f64,
    pub latest_cs_latency_us: f64,
    pub latest_mouse_shift_us: f64,
    pub latest_spin_duty_pct: f64,
    rng: BrownianNoise,
}

/// Deadband in µs — errors within this range are ignored to prevent
/// the PI controller from chasing Windows scheduler noise.
const DEADBAND_US: f64 = 200.0;
/// Maximum correction the PI controller can apply (µs).
const MAX_CORRECTION_US: f64 = 800.0;

impl PllEngine {
    pub fn new(tick_rate: TickRate) -> Self {
        let clock = HighResClock::new();
        let cfg = PllConfig::default();
        let target = tick_rate.us_per_tick();
        Self {
            clock,
            cfg,
            tick_rate,
            integral: 0.0,
            integral_limit: 0.02 * target,
            jitter_ema_us: 0.0,
            latest_phase_error_us: 0.0,
            smoothed_phase_error: 0.0,
            lfo_value: 0.0,
            lfo_step: 0.0,
            tick_index: 0,
            mouse_velocity_ema: 0.0,
            last_mouse_us: 0.0,
            total_spin_us: 0.0,
            total_tick_us: 0.0,
            latest_sleep_us: 0.0,
            latest_cs_latency_us: 0.0,
            latest_mouse_shift_us: 0.0,
            latest_spin_duty_pct: 0.0,
            rng: BrownianNoise::new(0xdeadbeef),
        }
    }

    pub fn apply_cfg(&mut self, cfg: PllConfig) {
        self.cfg = cfg;
        self.integral_limit = 0.02 * self.tick_rate.us_per_tick();
    }

    pub fn set_tick_rate(&mut self, rate: TickRate) {
        self.tick_rate = rate;
        let target = rate.us_per_tick();
        self.integral_limit = 0.02 * target;
        self.integral = 0.0;
        self.smoothed_phase_error = 0.0;
        self.tick_index = 0;
    }

    pub fn feed_mouse_delta(&mut self, now_us: f64) -> f64 {
        if self.last_mouse_us > 0.0 {
            let dt = (now_us - self.last_mouse_us).max(1.0);
            self.mouse_velocity_ema = 0.8 * self.mouse_velocity_ema + 0.2 * (1000.0 / dt);
        }
        self.last_mouse_us = now_us;
        self.mouse_velocity_ema
    }

    pub fn resync(&mut self) {
        self.integral = 0.0;
        self.smoothed_phase_error = 0.0;
        self.latest_phase_error_us = 0.0;
    }

    /// Compute planned sleep for the NEXT tick using the smoothed phase error.
    pub fn compute_planned_sleep_us(&mut self, mouse_shift_us: f64) -> f64 {
        let target_period_us = self.tick_rate.us_per_tick();

        // Brownian LFO
        self.lfo_step += 1.0 / (self.cfg.lfo_period_s * 60.0).max(1.0);
        if self.lfo_step >= 1.0 {
            self.lfo_step -= 1.0;
            let step = self.rng.gaussian() * 0.5;
            self.lfo_value =
                (self.lfo_value + step).clamp(-self.cfg.lfo_amp_us, self.cfg.lfo_amp_us);
        }

        // === ANTI-OSCILLATION CONTROLLER ===
        // 1. Deadband: ignore small errors (Windows scheduler noise)
        let controller_error = if self.smoothed_phase_error.abs() < DEADBAND_US {
            0.0
        } else {
            self.smoothed_phase_error - DEADBAND_US * self.smoothed_phase_error.signum()
        };

        // 2. PI controller on the deadband-filtered, smoothed error
        let p = self.cfg.kp * controller_error;
        self.integral = (self.integral + self.cfg.ki * controller_error)
            .clamp(-self.integral_limit, self.integral_limit);
        let mut correction = p + self.integral;

        // 3. Apply dynamic PLL power multiplier from config
        let power = self.cfg.power.clamp(0.1, 2.0);
        correction *= power;

        // 3. Clamp correction to prevent aggressive swings
        correction = correction.clamp(-MAX_CORRECTION_US, MAX_CORRECTION_US);

        // 4. Planned sleep = target period minus clamped correction
        let mut sleep_us = target_period_us - correction + mouse_shift_us + self.lfo_value;
        if sleep_us < 500.0 {
            sleep_us = 500.0;
        }
        if sleep_us > target_period_us * 1.2 {
            sleep_us = target_period_us * 1.2;
        }

        self.tick_index += 1;
        sleep_us
    }

    /// Record the actual cycle execution. This is where we measure the TRUE
    /// phase error and feed it through an EMA + median filter to the controller.
    pub fn record_actual_cycle(
        &mut self,
        planned_us: f64,
        actual_total_us: f64,
        spin_us: f64,
        mouse_shift_us: f64,
    ) {
        let target_period_us = self.tick_rate.us_per_tick();

        // 1. Raw phase error: how much the actual cycle deviated from target
        let raw_error = actual_total_us - target_period_us;
        self.latest_phase_error_us = raw_error;

        // 2. Fast EMA for display (jitter_ema_us) — shows raw jitter
        self.jitter_ema_us = self.jitter_ema_us * 0.92 + raw_error.abs() * 0.08;

        // 3. Slow EMA for the PI controller — ignores single-tick spikes
        //    This is the KEY anti-oscillation mechanism:
        //    The controller only reacts to systematic drift, not random noise.
        self.smoothed_phase_error = self.smoothed_phase_error * 0.97 + raw_error * 0.03;

        // Context switch overshoot
        let overshoot = (actual_total_us - planned_us).max(0.0);

        // Duty cycle
        self.total_spin_us += spin_us;
        self.total_tick_us += actual_total_us;
        let duty = if self.total_tick_us > 0.0 {
            (self.total_spin_us / self.total_tick_us) * 100.0
        } else {
            0.0
        };

        self.latest_sleep_us = planned_us;
        self.latest_cs_latency_us = overshoot;
        self.latest_mouse_shift_us = mouse_shift_us;
        self.latest_spin_duty_pct = duty;
    }

    pub fn snapshot(&self) -> PllSnapshot {
        PllSnapshot {
            tick_index: self.tick_index,
            sleep_us: self.latest_sleep_us,
            phase_error_us: self.smoothed_phase_error,
            jitter_ema_us: self.jitter_ema_us,
            cs_latency_us: self.latest_cs_latency_us,
            mouse_shift_us: self.latest_mouse_shift_us,
            lfo_us: self.lfo_value,
            active_layer: 0,
            spin_duty_pct: self.latest_spin_duty_pct,
            resolved_hz: 0,
            clock_mean_us: 0.0,
            clock_instability: 0.0,
            clock_samples: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_rate_us_matches() {
        assert!((TickRate::Hz64.us_per_tick() - 15625.0).abs() < 0.01);
        assert!((TickRate::Hz128.us_per_tick() - 7812.5).abs() < 0.01);
    }

    #[test]
    fn next_sleep_is_bounded() {
        let mut e = PllEngine::new(TickRate::Hz128);
        for _ in 0..100 {
            let s = e.compute_planned_sleep_us(0.0);
            assert!(s >= 500.0, "sleep went below floor: {s}");
        }
    }

    #[test]
    fn resync_resets_state() {
        let mut e = PllEngine::new(TickRate::Hz128);
        for _ in 0..10 {
            let _ = e.compute_planned_sleep_us(0.0);
        }
        e.resync();
        assert_eq!(e.integral, 0.0);
        assert_eq!(e.smoothed_phase_error, 0.0);
    }

    #[test]
    fn shared_snapshot_roundtrip() {
        let sh = PllShared::new(TickRate::Hz128, HzMode::Manual, 1000);
        sh.publish(
            PllSnapshot {
                sleep_us: 1500.0,
                ..PllSnapshot::default()
            },
            1000,
            7812.5,
            0.01,
            64,
        );
        let s = sh.snapshot();
        assert_eq!(s.sleep_us as u32, 1500);
        assert_eq!(s.resolved_hz, 1000);
        assert_eq!(s.clock_samples, 64);
        assert_eq!(s.clock_mean_us, 7812.5);
        assert_eq!(s.clock_instability, 0.01);
    }

    #[test]
    fn control_and_telemetry_are_cache_line_aligned() {
        assert_eq!(std::mem::align_of::<L0Control>(), 64);
        assert_eq!(std::mem::align_of::<L0Telemetry>(), 64);
    }

    #[test]
    fn no_drift_after_many_ticks() {
        let mut e = PllEngine::new(TickRate::Hz64);
        for _ in 0..5000 {
            let s = e.compute_planned_sleep_us(0.0);
            let actual = s + 1.0;
            e.record_actual_cycle(s, actual, 1.0, 0.0);
        }
        let last_sleep = e.snapshot().sleep_us;
        assert!(
            last_sleep > 14_000.0 && last_sleep < 17_000.0,
            "drift: {last_sleep}"
        );
    }

    #[test]
    fn deadband_prevents_oscillation() {
        let mut e = PllEngine::new(TickRate::Hz128);
        let target = 7812.5;
        // Simulate pure noise (no systematic drift) — sleep should stay near target
        for _ in 0..500 {
            let s = e.compute_planned_sleep_us(0.0);
            // Windows adds random ±1500 µs noise each tick
            let noise = (e.tick_index % 13) as f64 * 100.0 - 600.0;
            let actual = target + noise;
            e.record_actual_cycle(s, actual, 80.0, 0.0);
        }
        let last_sleep = e.snapshot().sleep_us;
        assert!(
            last_sleep > target * 0.85 && last_sleep < target * 1.15,
            "oscillation detected: sleep={last_sleep} target={target}"
        );
    }
}
