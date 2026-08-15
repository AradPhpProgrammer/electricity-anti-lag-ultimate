//! Low-overhead wake-up measurement and adaptive waiting.

use crate::platform::TimerResolutionGuard;
use std::collections::VecDeque;
use std::hint::spin_loop;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const RAW_CHANNEL_CAPACITY: usize = 4096;
const UI_CHANNEL_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Preset {
    Eco,
    #[default]
    Balanced,
    Precision,
}

impl Preset {
    pub const ALL: [Self; 3] = [Self::Eco, Self::Balanced, Self::Precision];

    pub fn label(self) -> &'static str {
        match self {
            Self::Eco => "Eco",
            Self::Balanced => "Balanced · Recommended",
            Self::Precision => "Precision Lab",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Eco => "Eco",
            Self::Balanced => "Balanced",
            Self::Precision => "Precision",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Eco => "2 ms observation with no active spin; lowest CPU cost.",
            Self::Balanced => "1 ms adaptive observation with a strict 3% spin budget.",
            Self::Precision => "0.5 ms lab profile for short A/B tests; higher CPU budget.",
        }
    }

    pub fn config(self) -> MonitorConfig {
        match self {
            Self::Eco => MonitorConfig {
                preset: self,
                interval_us: 2_000,
                adaptive_spin: false,
                initial_spin_us: 0,
                min_spin_us: 0,
                max_spin_us: 0,
                safety_margin_us: 0,
                controller_alpha: 0.20,
                spin_budget_pct: 0.0,
                target_p99_us: 200.0,
                request_timer_resolution: false,
                telemetry_period_ms: 50,
                window_samples: 2_048,
            },
            Self::Balanced => MonitorConfig {
                preset: self,
                interval_us: 1_000,
                adaptive_spin: true,
                initial_spin_us: 30,
                min_spin_us: 10,
                max_spin_us: 250,
                safety_margin_us: 15,
                controller_alpha: 0.20,
                spin_budget_pct: 3.0,
                target_p99_us: 100.0,
                request_timer_resolution: true,
                telemetry_period_ms: 50,
                window_samples: 2_048,
            },
            Self::Precision => MonitorConfig {
                preset: self,
                interval_us: 500,
                adaptive_spin: true,
                initial_spin_us: 40,
                min_spin_us: 15,
                max_spin_us: 200,
                safety_margin_us: 12,
                controller_alpha: 0.25,
                spin_budget_pct: 8.0,
                target_p99_us: 50.0,
                request_timer_resolution: true,
                telemetry_period_ms: 50,
                window_samples: 4_096,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct MonitorConfig {
    pub preset: Preset,
    pub interval_us: u64,
    pub adaptive_spin: bool,
    pub initial_spin_us: u32,
    pub min_spin_us: u32,
    pub max_spin_us: u32,
    pub safety_margin_us: u32,
    pub controller_alpha: f64,
    pub spin_budget_pct: f64,
    pub target_p99_us: f64,
    pub request_timer_resolution: bool,
    pub telemetry_period_ms: u64,
    pub window_samples: usize,
}

impl MonitorConfig {
    pub fn sanitized(mut self) -> Self {
        self.interval_us = self.interval_us.clamp(250, 10_000);
        self.telemetry_period_ms = self.telemetry_period_ms.clamp(25, 1_000);
        self.window_samples = self.window_samples.clamp(128, 65_536);
        self.controller_alpha = self.controller_alpha.clamp(0.01, 1.0);
        self.spin_budget_pct = self.spin_budget_pct.clamp(0.0, 50.0);
        self.max_spin_us = self
            .max_spin_us
            .min((self.interval_us.saturating_mul(4) / 5) as u32);
        self.min_spin_us = self.min_spin_us.min(self.max_spin_us);
        self.initial_spin_us = self
            .initial_spin_us
            .clamp(self.min_spin_us, self.max_spin_us);
        if !self.adaptive_spin {
            self.initial_spin_us = 0;
            self.min_spin_us = 0;
            self.max_spin_us = 0;
            self.spin_budget_pct = 0.0;
        }
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimingHealth {
    Calibrating,
    Stable,
    Variable,
    HighTail,
}

impl TimingHealth {
    pub fn label(self) -> &'static str {
        match self {
            Self::Calibrating => "Calibrating",
            Self::Stable => "Stable",
            Self::Variable => "Variable",
            Self::HighTail => "High tail",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TelemetryPoint {
    pub elapsed_secs: f64,
    pub latest_error_us: f64,
    pub p50_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub p999_us: f64,
    pub max_us: f64,
    pub mad_us: f64,
    pub sleep_overshoot_p95_us: f64,
    pub spin_window_us: u32,
    pub spin_duty_pct: f64,
    pub total_samples: u64,
    pub total_missed_deadlines: u64,
    pub telemetry_dropped: u64,
    pub timer_request_active: bool,
    pub spike: bool,
    pub health: TimingHealth,
    pub recommendation: &'static str,
}

#[derive(Clone, Debug)]
pub enum MonitorEvent {
    Point(TelemetryPoint),
    Stopped(StopSummary),
}

#[derive(Clone, Debug, Default)]
pub struct StopSummary {
    pub total_samples: u64,
    pub total_missed_deadlines: u64,
    pub telemetry_dropped: u64,
    pub session_max_us: f64,
}

pub struct MonitorSession {
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    aggregator: Option<JoinHandle<StopSummary>>,
}

impl MonitorSession {
    pub fn start(config: MonitorConfig) -> (Self, Receiver<MonitorEvent>) {
        let config = config.sanitized();
        let (raw_tx, raw_rx) = sync_channel(RAW_CHANNEL_CAPACITY);
        let (ui_tx, ui_rx) = sync_channel(UI_CHANNEL_CAPACITY);
        let running = Arc::new(AtomicBool::new(true));
        let spin_window = Arc::new(AtomicU32::new(config.initial_spin_us));
        let dropped = Arc::new(AtomicU64::new(0));
        let timer_active = Arc::new(AtomicBool::new(false));

        let aggregator = {
            let spin_window = Arc::clone(&spin_window);
            let dropped = Arc::clone(&dropped);
            let timer_active = Arc::clone(&timer_active);
            let config = config.clone();
            thread::Builder::new()
                .name("hermes-telemetry".to_owned())
                .spawn(move || {
                    aggregate_loop(config, raw_rx, ui_tx, spin_window, dropped, timer_active)
                })
                .expect("failed to start Hermes telemetry thread")
        };

        let worker = {
            let running = Arc::clone(&running);
            let spin_window = Arc::clone(&spin_window);
            let dropped = Arc::clone(&dropped);
            let timer_active = Arc::clone(&timer_active);
            thread::Builder::new()
                .name("hermes-timing".to_owned())
                .spawn(move || {
                    measurement_loop(config, raw_tx, running, spin_window, dropped, timer_active)
                })
                .expect("failed to start Hermes timing thread")
        };

        (
            Self {
                running,
                worker: Some(worker),
                aggregator: Some(aggregator),
            },
            ui_rx,
        )
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    pub fn stop(&mut self) -> StopSummary {
        self.running.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.aggregator
            .take()
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default()
    }
}

impl Drop for MonitorSession {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct WaitObservation {
    pub actual: Instant,
    pub wake_error_us: f64,
    pub sleep_overshoot_us: f64,
    pub active_spin_us: f64,
}

pub(crate) fn wait_until(deadline: Instant, spin_window_us: u32) -> WaitObservation {
    let spin_duration = Duration::from_micros(u64::from(spin_window_us));
    let sleep_target = deadline.checked_sub(spin_duration).unwrap_or(deadline);
    let before_sleep = Instant::now();

    if before_sleep < sleep_target {
        thread::sleep(sleep_target.duration_since(before_sleep));
    }

    let after_sleep = Instant::now();
    let sleep_overshoot_us = after_sleep
        .saturating_duration_since(sleep_target)
        .as_secs_f64()
        * 1_000_000.0;
    let mut active_spin_us = 0.0;

    if after_sleep < deadline {
        let spin_started = after_sleep;
        while Instant::now() < deadline {
            spin_loop();
        }
        active_spin_us = Instant::now()
            .saturating_duration_since(spin_started)
            .as_secs_f64()
            * 1_000_000.0;
    }

    let actual = Instant::now();
    let wake_error_us = actual.saturating_duration_since(deadline).as_secs_f64() * 1_000_000.0;

    WaitObservation {
        actual,
        wake_error_us,
        sleep_overshoot_us,
        active_spin_us,
    }
}

pub(crate) fn advance_deadline(
    deadline: Instant,
    actual: Instant,
    interval: Duration,
) -> (Instant, u64) {
    let interval_ns = interval.as_nanos().max(1);
    let late_ns = actual.saturating_duration_since(deadline).as_nanos();
    let missed = (late_ns / interval_ns) as u64;
    let periods = missed.saturating_add(1);
    let advance = Duration::from_secs_f64(interval.as_secs_f64() * periods as f64);
    (deadline + advance, missed)
}

#[derive(Clone, Copy, Debug)]
struct RawSample {
    elapsed_secs: f64,
    wake_error_us: f64,
    sleep_overshoot_us: f64,
    active_spin_us: f64,
    missed_deadlines: u64,
}

fn measurement_loop(
    config: MonitorConfig,
    raw_tx: SyncSender<RawSample>,
    running: Arc<AtomicBool>,
    spin_window: Arc<AtomicU32>,
    dropped: Arc<AtomicU64>,
    timer_active: Arc<AtomicBool>,
) {
    let timer_guard = TimerResolutionGuard::request(config.request_timer_resolution);
    timer_active.store(timer_guard.is_active(), Ordering::Release);

    let interval = Duration::from_micros(config.interval_us);
    let started = Instant::now();
    let mut deadline = started + interval;

    while running.load(Ordering::Acquire) {
        let active_spin = if config.adaptive_spin {
            spin_window.load(Ordering::Relaxed)
        } else {
            0
        };
        let observation = wait_until(deadline, active_spin);
        let (next_deadline, missed_deadlines) =
            advance_deadline(deadline, observation.actual, interval);

        let sample = RawSample {
            elapsed_secs: observation.actual.duration_since(started).as_secs_f64(),
            wake_error_us: observation.wake_error_us,
            sleep_overshoot_us: observation.sleep_overshoot_us,
            active_spin_us: observation.active_spin_us,
            missed_deadlines,
        };

        match raw_tx.try_send(sample) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => break,
        }
        deadline = next_deadline;
    }
}

fn aggregate_loop(
    config: MonitorConfig,
    raw_rx: Receiver<RawSample>,
    ui_tx: SyncSender<MonitorEvent>,
    spin_window: Arc<AtomicU32>,
    dropped: Arc<AtomicU64>,
    timer_active: Arc<AtomicBool>,
) -> StopSummary {
    let mut recent = VecDeque::with_capacity(config.window_samples);
    let mut total_samples = 0_u64;
    let mut total_missed = 0_u64;
    let mut session_max_us = 0.0_f64;
    let mut last_emit = Instant::now();
    let emit_period = Duration::from_millis(config.telemetry_period_ms);
    let mut controller = AdaptiveSpinController::new(&config);
    let mut disconnected = false;

    loop {
        match raw_rx.recv_timeout(Duration::from_millis(10)) {
            Ok(sample) => {
                total_samples += 1;
                total_missed = total_missed.saturating_add(sample.missed_deadlines);
                session_max_us = session_max_us.max(sample.wake_error_us);
                if recent.len() == config.window_samples {
                    recent.pop_front();
                }
                recent.push_back(sample);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => disconnected = true,
        }

        if last_emit.elapsed() >= emit_period && !recent.is_empty() {
            let point = build_point(
                &config,
                &recent,
                total_samples,
                total_missed,
                dropped.load(Ordering::Relaxed),
                timer_active.load(Ordering::Acquire),
                &mut controller,
            );
            spin_window.store(point.spin_window_us, Ordering::Relaxed);
            let _ = ui_tx.try_send(MonitorEvent::Point(point));
            last_emit = Instant::now();
        }

        if disconnected {
            break;
        }
    }

    let summary = StopSummary {
        total_samples,
        total_missed_deadlines: total_missed,
        telemetry_dropped: dropped.load(Ordering::Relaxed),
        session_max_us,
    };
    let _ = ui_tx.try_send(MonitorEvent::Stopped(summary.clone()));
    summary
}

fn build_point(
    config: &MonitorConfig,
    recent: &VecDeque<RawSample>,
    total_samples: u64,
    total_missed: u64,
    telemetry_dropped: u64,
    timer_request_active: bool,
    controller: &mut AdaptiveSpinController,
) -> TelemetryPoint {
    let wake_errors: Vec<f64> = recent.iter().map(|sample| sample.wake_error_us).collect();
    let sleep_overshoots: Vec<f64> = recent
        .iter()
        .map(|sample| sample.sleep_overshoot_us)
        .collect();
    let active_spin_total: f64 = recent.iter().map(|sample| sample.active_spin_us).sum();
    let spin_duty_pct = if recent.is_empty() {
        0.0
    } else {
        active_spin_total / (recent.len() as f64 * config.interval_us as f64) * 100.0
    };
    let stats = Distribution::from_values(&wake_errors);
    let sleep_stats = Distribution::from_values(&sleep_overshoots);

    let spin_window_us = if config.adaptive_spin {
        controller.update(
            sleep_stats.p95,
            stats.p99,
            spin_duty_pct,
            config.target_p99_us,
        )
    } else {
        0
    };

    let latest = recent.back().copied().unwrap_or(RawSample {
        elapsed_secs: 0.0,
        wake_error_us: 0.0,
        sleep_overshoot_us: 0.0,
        active_spin_us: 0.0,
        missed_deadlines: 0,
    });
    let robust_z = if stats.mad > f64::EPSILON {
        0.6745 * (latest.wake_error_us - stats.p50) / stats.mad
    } else if latest.wake_error_us > stats.p50 + 50.0 {
        f64::INFINITY
    } else {
        0.0
    };
    let spike = robust_z > 6.0 || latest.missed_deadlines > 0;
    let health = classify_health(config, recent.len(), stats.p99);
    let recommendation = recommendation_for(
        config,
        health,
        spin_duty_pct,
        telemetry_dropped,
        total_missed,
    );

    TelemetryPoint {
        elapsed_secs: latest.elapsed_secs,
        latest_error_us: latest.wake_error_us,
        p50_us: stats.p50,
        p95_us: stats.p95,
        p99_us: stats.p99,
        p999_us: stats.p999,
        max_us: stats.max,
        mad_us: stats.mad,
        sleep_overshoot_p95_us: sleep_stats.p95,
        spin_window_us,
        spin_duty_pct,
        total_samples,
        total_missed_deadlines: total_missed,
        telemetry_dropped,
        timer_request_active,
        spike,
        health,
        recommendation,
    }
}

fn classify_health(config: &MonitorConfig, samples: usize, p99_us: f64) -> TimingHealth {
    if samples < 128 {
        TimingHealth::Calibrating
    } else if p99_us <= config.target_p99_us {
        TimingHealth::Stable
    } else if p99_us <= config.interval_us as f64 * 0.5 {
        TimingHealth::Variable
    } else {
        TimingHealth::HighTail
    }
}

fn recommendation_for(
    config: &MonitorConfig,
    health: TimingHealth,
    spin_duty_pct: f64,
    telemetry_dropped: u64,
    total_missed: u64,
) -> &'static str {
    if telemetry_dropped > 0 {
        "Telemetry backpressure detected. Use Eco or close heavy background workloads."
    } else if config.adaptive_spin && spin_duty_pct > config.spin_budget_pct * 1.15 {
        "CPU spin budget is saturated. Balanced/Eco is safer for long sessions."
    } else if total_missed > 0 && health == TimingHealth::HighTail {
        "Large scheduler tails detected. Re-test idle, then inspect IRQ/DPC with WPA or timerlat."
    } else {
        match health {
            TimingHealth::Calibrating => {
                "Keep the window focused and let at least 128 samples accumulate."
            }
            TimingHealth::Stable => "Current preset is within its p99 target; keep it unchanged.",
            TimingHealth::Variable => {
                "Repeat an equal-duration A/B benchmark before changing system settings."
            }
            TimingHealth::HighTail => {
                "Use Precision only for a short A/B test; investigate the OS tail before tuning."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Distribution {
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub p999: f64,
    pub max: f64,
    pub mean: f64,
    pub mad: f64,
}

impl Distribution {
    pub fn from_values(values: &[f64]) -> Self {
        if values.is_empty() {
            return Self::default();
        }
        let mut sorted: Vec<f64> = values
            .iter()
            .copied()
            .filter(|value| value.is_finite())
            .collect();
        if sorted.is_empty() {
            return Self::default();
        }
        sorted.sort_by(f64::total_cmp);
        let p50 = percentile_sorted(&sorted, 0.50);
        let deviations: Vec<f64> = sorted.iter().map(|value| (value - p50).abs()).collect();
        let mut deviations = deviations;
        deviations.sort_by(f64::total_cmp);

        Self {
            p50,
            p95: percentile_sorted(&sorted, 0.95),
            p99: percentile_sorted(&sorted, 0.99),
            p999: percentile_sorted(&sorted, 0.999),
            max: *sorted.last().unwrap_or(&0.0),
            mean: sorted.iter().sum::<f64>() / sorted.len() as f64,
            mad: percentile_sorted(&deviations, 0.50),
        }
    }
}

fn percentile_sorted(sorted: &[f64], quantile: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let position = quantile.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let lower = position.floor() as usize;
    let upper = position.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let weight = position - lower as f64;
        sorted[lower] * (1.0 - weight) + sorted[upper] * weight
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AdaptiveSpinController {
    current_us: f64,
    min_us: f64,
    max_us: f64,
    safety_margin_us: f64,
    alpha: f64,
    budget_pct: f64,
}

impl AdaptiveSpinController {
    pub(crate) fn new(config: &MonitorConfig) -> Self {
        Self {
            current_us: f64::from(config.initial_spin_us),
            min_us: f64::from(config.min_spin_us),
            max_us: f64::from(config.max_spin_us),
            safety_margin_us: f64::from(config.safety_margin_us),
            alpha: config.controller_alpha,
            budget_pct: config.spin_budget_pct,
        }
    }

    pub(crate) fn update(
        &mut self,
        sleep_overshoot_p95_us: f64,
        wake_p99_us: f64,
        spin_duty_pct: f64,
        target_p99_us: f64,
    ) -> u32 {
        let mut candidate =
            (sleep_overshoot_p95_us + self.safety_margin_us).clamp(self.min_us, self.max_us);

        if self.budget_pct > 0.0 && spin_duty_pct > self.budget_pct {
            let budget_ratio = (self.budget_pct / spin_duty_pct).clamp(0.35, 1.0);
            candidate = (self.current_us * budget_ratio).max(self.min_us);
        } else if wake_p99_us > target_p99_us && spin_duty_pct < self.budget_pct * 0.9 {
            candidate = candidate.max(self.current_us + self.safety_margin_us * 0.5);
        }

        self.current_us = ((1.0 - self.alpha) * self.current_us + self.alpha * candidate)
            .clamp(self.min_us, self.max_us);
        self.current_us.round() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_preserves_tail_outlier() {
        let mut values = vec![10.0; 999];
        values.push(10_000.0);
        let stats = Distribution::from_values(&values);
        assert_eq!(stats.max, 10_000.0);
        assert!(stats.p999 > 10.0);
    }

    #[test]
    fn balanced_is_the_conservative_default() {
        let config = Preset::default().config();
        assert_eq!(config.preset, Preset::Balanced);
        assert_eq!(config.interval_us, 1_000);
        assert!(config.adaptive_spin);
        assert!(config.spin_budget_pct <= 3.0);
    }

    #[test]
    fn controller_moves_toward_observed_overshoot() {
        let mut config = Preset::Balanced.config();
        config.spin_budget_pct = 100.0;
        let mut controller = AdaptiveSpinController::new(&config);
        let before = controller.current_us;
        let after = controller.update(120.0, 200.0, 1.0, 100.0);
        assert!(f64::from(after) > before);
        assert!(after <= config.max_spin_us);
    }

    #[test]
    fn controller_respects_cpu_budget() {
        let mut config = Preset::Balanced.config();
        config.initial_spin_us = 100;
        let mut controller = AdaptiveSpinController::new(&config);
        let after = controller.update(150.0, 10.0, 20.0, 100.0);
        assert!(after < 100);
        assert!(after >= config.min_spin_us);
    }

    #[test]
    fn session_starts_emits_and_stops() {
        let mut config = Preset::Eco.config();
        config.interval_us = 1_000;
        config.telemetry_period_ms = 25;
        let (mut session, receiver) = MonitorSession::start(config);
        let event = receiver.recv_timeout(Duration::from_secs(1));
        assert!(matches!(event, Ok(MonitorEvent::Point(_))));
        let summary = session.stop();
        assert!(summary.total_samples > 0);
    }
}
