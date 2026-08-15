//! Equal-duration A/B benchmark for sleep-only versus adaptive waiting.

use crate::platform::{platform_name, TimerResolutionGuard};
use crate::telemetry::{
    advance_deadline, wait_until, AdaptiveSpinController, Distribution, MonitorConfig, Preset,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Strategy {
    SleepOnly,
    Adaptive,
}

#[derive(Clone, Debug)]
pub struct BenchmarkResult {
    pub strategy: &'static str,
    pub samples: u64,
    pub missed_deadlines: u64,
    pub p50_us: f64,
    pub p95_us: f64,
    pub p99_us: f64,
    pub p999_us: f64,
    pub max_us: f64,
    pub mean_us: f64,
    pub spin_duty_pct: f64,
    pub final_spin_window_us: u32,
}

#[derive(Clone, Debug)]
pub struct BenchmarkReport {
    pub platform: &'static str,
    pub preset: Preset,
    pub interval_us: u64,
    pub seconds_per_run: f64,
    pub rounds: u32,
    pub timer_request_active: bool,
    pub round_results: Vec<RoundResult>,
    pub sleep_only: BenchmarkResult,
    pub adaptive: BenchmarkResult,
}

#[derive(Clone, Debug)]
pub struct RoundResult {
    pub round: u32,
    pub first_strategy: &'static str,
    pub sleep_p99_us: f64,
    pub adaptive_p99_us: f64,
    pub p99_improvement_pct: f64,
}

impl BenchmarkReport {
    pub fn p50_improvement_pct(&self) -> f64 {
        relative_improvement(self.sleep_only.p50_us, self.adaptive.p50_us)
    }

    pub fn p99_improvement_pct(&self) -> f64 {
        relative_improvement(self.sleep_only.p99_us, self.adaptive.p99_us)
    }

    pub fn p999_improvement_pct(&self) -> f64 {
        relative_improvement(self.sleep_only.p999_us, self.adaptive.p999_us)
    }

    pub fn max_improvement_pct(&self) -> f64 {
        relative_improvement(self.sleep_only.max_us, self.adaptive.max_us)
    }

    pub fn adaptive_p99_wins(&self) -> usize {
        self.round_results
            .iter()
            .filter(|round| round.p99_improvement_pct > 0.0)
            .count()
    }

    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{\n",
                "  \"schema_version\": 2,\n",
                "  \"tool_version\": \"{}\",\n",
                "  \"platform\": \"{}\",\n",
                "  \"preset\": \"{}\",\n",
                "  \"interval_us\": {},\n",
                "  \"seconds_per_run\": {:.3},\n",
                "  \"rounds\": {},\n",
                "  \"timer_request_active\": {},\n",
                "  \"round_results\": {},\n",
                "  \"sleep_only\": {},\n",
                "  \"adaptive\": {},\n",
                "  \"improvement_pct\": {{\"p50\": {:.3}, \"p99\": {:.3}, \"p999\": {:.3}, \"max\": {:.3}}}\n",
                "}}"
            ),
            crate::VERSION,
            self.platform,
            self.preset.short_label(),
            self.interval_us,
            self.seconds_per_run,
            self.rounds,
            self.timer_request_active,
            rounds_json(&self.round_results),
            result_json(&self.sleep_only),
            result_json(&self.adaptive),
            self.p50_improvement_pct(),
            self.p99_improvement_pct(),
            self.p999_improvement_pct(),
            self.max_improvement_pct(),
        )
    }
}

pub fn run_comparison(
    preset: Preset,
    interval_us: u64,
    seconds_per_run: f64,
    rounds: u32,
) -> BenchmarkReport {
    let mut config = preset.config();
    config.interval_us = interval_us;
    let config = config.sanitized();
    let run_duration = Duration::from_secs_f64(seconds_per_run.clamp(0.25, 3_600.0));
    let rounds = rounds.clamp(1, 100);
    let timer_guard = TimerResolutionGuard::request(config.request_timer_resolution);

    let mut sleep_samples = StrategySamples::default();
    let mut adaptive_samples = StrategySamples::default();
    let mut round_results = Vec::with_capacity(rounds as usize);

    for round in 0..rounds {
        let sleep_first = round % 2 == 0;
        let (sleep_run, adaptive_run) = if sleep_first {
            let sleep_run = run_once(Strategy::SleepOnly, &config, run_duration);
            thread::sleep(Duration::from_millis(75));
            let adaptive_run = run_once(Strategy::Adaptive, &config, run_duration);
            (sleep_run, adaptive_run)
        } else {
            let adaptive_run = run_once(Strategy::Adaptive, &config, run_duration);
            thread::sleep(Duration::from_millis(75));
            let sleep_run = run_once(Strategy::SleepOnly, &config, run_duration);
            (sleep_run, adaptive_run)
        };
        let sleep_result = sleep_run.summarize("sleep-only", config.interval_us);
        let adaptive_result = adaptive_run.summarize("adaptive", config.interval_us);
        round_results.push(RoundResult {
            round: round + 1,
            first_strategy: if sleep_first {
                "sleep-only"
            } else {
                "adaptive"
            },
            sleep_p99_us: sleep_result.p99_us,
            adaptive_p99_us: adaptive_result.p99_us,
            p99_improvement_pct: relative_improvement(sleep_result.p99_us, adaptive_result.p99_us),
        });
        sleep_samples.append(sleep_run);
        adaptive_samples.append(adaptive_run);
        if round + 1 < rounds {
            thread::sleep(Duration::from_millis(150));
        }
    }

    BenchmarkReport {
        platform: platform_name(),
        preset,
        interval_us: config.interval_us,
        seconds_per_run: run_duration.as_secs_f64(),
        rounds,
        timer_request_active: timer_guard.is_active(),
        round_results,
        sleep_only: sleep_samples.summarize("sleep-only", config.interval_us),
        adaptive: adaptive_samples.summarize("adaptive", config.interval_us),
    }
}

#[derive(Default)]
struct StrategySamples {
    wake_errors: Vec<f64>,
    sleep_overshoots: Vec<f64>,
    active_spin_us: Vec<f64>,
    missed_deadlines: u64,
    final_spin_window_us: u32,
}

impl StrategySamples {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            wake_errors: Vec::with_capacity(capacity),
            sleep_overshoots: Vec::with_capacity(capacity),
            active_spin_us: Vec::with_capacity(capacity),
            missed_deadlines: 0,
            final_spin_window_us: 0,
        }
    }

    fn append(&mut self, mut other: Self) {
        self.wake_errors.append(&mut other.wake_errors);
        self.sleep_overshoots.append(&mut other.sleep_overshoots);
        self.active_spin_us.append(&mut other.active_spin_us);
        self.missed_deadlines = self.missed_deadlines.saturating_add(other.missed_deadlines);
        self.final_spin_window_us = other.final_spin_window_us;
    }

    fn summarize(&self, strategy: &'static str, interval_us: u64) -> BenchmarkResult {
        let stats = Distribution::from_values(&self.wake_errors);
        let spin_total: f64 = self.active_spin_us.iter().sum();
        let spin_duty_pct = if self.active_spin_us.is_empty() {
            0.0
        } else {
            spin_total / (self.active_spin_us.len() as f64 * interval_us as f64) * 100.0
        };

        BenchmarkResult {
            strategy,
            samples: self.wake_errors.len() as u64,
            missed_deadlines: self.missed_deadlines,
            p50_us: stats.p50,
            p95_us: stats.p95,
            p99_us: stats.p99,
            p999_us: stats.p999,
            max_us: stats.max,
            mean_us: stats.mean,
            spin_duty_pct,
            final_spin_window_us: self.final_spin_window_us,
        }
    }
}

fn run_once(strategy: Strategy, config: &MonitorConfig, duration: Duration) -> StrategySamples {
    let interval = Duration::from_micros(config.interval_us);
    let started = Instant::now();
    let end = started + duration;
    let mut deadline = started + interval;
    let expected_samples = (duration.as_nanos() / interval.as_nanos().max(1)) as usize + 16;
    let mut samples = StrategySamples::with_capacity(expected_samples);
    let adaptive = strategy == Strategy::Adaptive && config.adaptive_spin;
    let controller = if adaptive {
        Some(ControllerHarness::start(config.clone()))
    } else {
        None
    };

    while Instant::now() < end {
        let spin_window_us = controller
            .as_ref()
            .map(ControllerHarness::spin_window_us)
            .unwrap_or(0);
        let observation = wait_until(deadline, spin_window_us);
        let (next_deadline, missed) = advance_deadline(deadline, observation.actual, interval);
        samples.wake_errors.push(observation.wake_error_us);
        samples
            .sleep_overshoots
            .push(observation.sleep_overshoot_us);
        samples.active_spin_us.push(observation.active_spin_us);
        samples.missed_deadlines = samples.missed_deadlines.saturating_add(missed);
        deadline = next_deadline;

        if let Some(controller) = controller.as_ref() {
            controller.observe(ControlSample {
                wake_error_us: observation.wake_error_us,
                sleep_overshoot_us: observation.sleep_overshoot_us,
                active_spin_us: observation.active_spin_us,
            });
        }
    }

    samples.final_spin_window_us = controller
        .map(ControllerHarness::finish)
        .unwrap_or_default();
    samples
}

#[derive(Clone, Copy)]
struct ControlSample {
    wake_error_us: f64,
    sleep_overshoot_us: f64,
    active_spin_us: f64,
}

struct ControllerHarness {
    sender: SyncSender<ControlSample>,
    spin_window: Arc<AtomicU32>,
    dropped: Arc<AtomicU64>,
    thread: JoinHandle<u32>,
}

impl ControllerHarness {
    fn start(config: MonitorConfig) -> Self {
        let (sender, receiver) = sync_channel(4_096);
        let spin_window = Arc::new(AtomicU32::new(config.initial_spin_us));
        let dropped = Arc::new(AtomicU64::new(0));
        let thread_spin = Arc::clone(&spin_window);
        let thread = thread::Builder::new()
            .name("hermes-bench-controller".to_owned())
            .spawn(move || controller_loop(config, receiver, thread_spin))
            .expect("failed to start benchmark controller");
        Self {
            sender,
            spin_window,
            dropped,
            thread,
        }
    }

    fn spin_window_us(&self) -> u32 {
        self.spin_window.load(Ordering::Relaxed)
    }

    fn observe(&self, sample: ControlSample) {
        match self.sender.try_send(sample) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    fn finish(self) -> u32 {
        let ControllerHarness {
            sender,
            spin_window,
            dropped: _,
            thread,
        } = self;
        drop(sender);
        thread
            .join()
            .unwrap_or_else(|_| spin_window.load(Ordering::Relaxed))
    }
}

fn controller_loop(
    config: MonitorConfig,
    receiver: Receiver<ControlSample>,
    spin_window: Arc<AtomicU32>,
) -> u32 {
    let mut recent = VecDeque::with_capacity(1_024);
    let mut controller = AdaptiveSpinController::new(&config);
    let mut last_update = Instant::now();

    loop {
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(sample) => {
                if recent.len() == 1_024 {
                    recent.pop_front();
                }
                recent.push_back(sample);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if recent.len() >= 64 && last_update.elapsed() >= Duration::from_millis(50) {
            let wake_errors: Vec<f64> = recent.iter().map(|sample| sample.wake_error_us).collect();
            let overshoots: Vec<f64> = recent
                .iter()
                .map(|sample| sample.sleep_overshoot_us)
                .collect();
            let spin_total: f64 = recent.iter().map(|sample| sample.active_spin_us).sum();
            let spin_duty_pct =
                spin_total / (recent.len() as f64 * config.interval_us as f64) * 100.0;
            let wake_stats = Distribution::from_values(&wake_errors);
            let sleep_stats = Distribution::from_values(&overshoots);
            let next = controller.update(
                sleep_stats.p95,
                wake_stats.p99,
                spin_duty_pct,
                config.target_p99_us,
            );
            spin_window.store(next, Ordering::Relaxed);
            last_update = Instant::now();
        }
    }

    spin_window.load(Ordering::Relaxed)
}

fn relative_improvement(baseline: f64, candidate: f64) -> f64 {
    if baseline <= f64::EPSILON {
        0.0
    } else {
        (baseline - candidate) / baseline * 100.0
    }
}

fn result_json(result: &BenchmarkResult) -> String {
    format!(
        concat!(
            "{{\"strategy\":\"{}\",\"samples\":{},\"missed_deadlines\":{},",
            "\"p50_us\":{:.3},\"p95_us\":{:.3},\"p99_us\":{:.3},",
            "\"p999_us\":{:.3},\"max_us\":{:.3},\"mean_us\":{:.3},",
            "\"spin_duty_pct\":{:.3},\"final_spin_window_us\":{}}}"
        ),
        result.strategy,
        result.samples,
        result.missed_deadlines,
        result.p50_us,
        result.p95_us,
        result.p99_us,
        result.p999_us,
        result.max_us,
        result.mean_us,
        result.spin_duty_pct,
        result.final_spin_window_us,
    )
}

fn rounds_json(rounds: &[RoundResult]) -> String {
    let entries: Vec<String> = rounds
        .iter()
        .map(|round| {
            format!(
                concat!(
                    "{{\"round\":{},\"first_strategy\":\"{}\",",
                    "\"sleep_p99_us\":{:.3},\"adaptive_p99_us\":{:.3},",
                    "\"p99_improvement_pct\":{:.3}}}"
                ),
                round.round,
                round.first_strategy,
                round.sleep_p99_us,
                round.adaptive_p99_us,
                round.p99_improvement_pct,
            )
        })
        .collect();
    format!("[{}]", entries.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn improvement_sign_is_honest() {
        assert_eq!(relative_improvement(100.0, 50.0), 50.0);
        assert_eq!(relative_improvement(100.0, 125.0), -25.0);
    }

    #[test]
    fn json_contains_tail_metrics() {
        let result = BenchmarkResult {
            strategy: "test",
            samples: 10,
            missed_deadlines: 1,
            p50_us: 1.0,
            p95_us: 2.0,
            p99_us: 3.0,
            p999_us: 4.0,
            max_us: 5.0,
            mean_us: 2.0,
            spin_duty_pct: 1.0,
            final_spin_window_us: 10,
        };
        let json = result_json(&result);
        assert!(json.contains("\"p999_us\":4.000"));
        assert!(json.contains("\"missed_deadlines\":1"));
    }
}
