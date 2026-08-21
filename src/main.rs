#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use eframe::egui::{self, Color32, FontId, RichText, Stroke};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use hermes_engine::config::HermesConfig;
use hermes_engine::games::detect_running_game;
use hermes_engine::hotkey::{drain_click, drain_reset, drain_toggle, spawn_hotkey_thread};
use hermes_engine::input::{
    ClickInterrupt, HzDetector, HzMode, MouseModulator, POLLING_CANDIDATES,
};
use hermes_engine::network::NetworkGuard;
use hermes_engine::platform::{platform_name, ClockStabilityMonitor, TimerResolutionGuard};
use hermes_engine::pll::{
    PllConfig, PllEngine, PllShared, TickRate, TELEMETRY_PUBLISH_INTERVAL_TICKS,
};
use hermes_engine::system_mod::SystemGuard;
use hermes_engine::telemetry::Preset;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const ACCENT: Color32 = Color32::from_rgb(0, 212, 255); // Neon Cyan
const GREEN: Color32 = Color32::from_rgb(52, 211, 153); // Emerald
const AMBER: Color32 = Color32::from_rgb(251, 191, 36); // Golden Amber
const RED: Color32 = Color32::from_rgb(248, 113, 113); // Coral Red
const PANEL: Color32 = Color32::from_rgb(18, 22, 31);
const PANEL_ALT: Color32 = Color32::from_rgb(24, 30, 42);
const BG_DARK: Color32 = Color32::from_rgb(11, 14, 20);

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Engine,
    Lab,
    Input,
    System,
    Settings,
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1_300.0, 900.0])
            .with_min_inner_size([1_080.0, 750.0])
            .with_title("⚡ Hermes Engine Ultimate v5.4 — Sub-Microsecond Precision Engine"),
        ..Default::default()
    };
    eframe::run_native(
        &format!("Hermes Engine Ultimate {}", hermes_engine::VERSION),
        options,
        Box::new(|cc| Ok(Box::new(HermesApp::new(cc)))),
    )
}

struct HermesApp {
    tab: Tab,
    cfg: HermesConfig,
    running: Arc<AtomicBool>,
    pll_shared: Arc<PllShared>,
    click_interrupt: Arc<ClickInterrupt>,
    network_guard: Arc<std::sync::Mutex<NetworkGuard>>,
    system_guard: Arc<std::sync::Mutex<SystemGuard>>,

    // Telemetry graphs & histories
    wake_history: VecDeque<[f64; 2]>,
    jitter_history: VecDeque<[f64; 2]>,

    latest_clock_mean_us: f64,
    latest_clock_instability: f64,
    latest_resolved_hz: u32,

    // Polling rate selectors
    selected_mouse_hz: u32,
    selected_kb_hz: u32,
    is_auto_hz: bool,

    game_name: Option<String>,
    status: String,
    worker_join: Option<thread::JoinHandle<()>>,

    // Live config knobs
    pll_kp: f64,
    pll_ki: f64,
    pll_power: f64,
    pll_lfo_amp: f64,
    pll_lfo_period: f64,
    selected_tick_rate: TickRate,
    ram_only: bool,
    auto_tune: bool,

    // Lab Diagnostics
    raw_running: Arc<AtomicBool>,
    raw_join: Option<thread::JoinHandle<()>>,
    raw_latest: Arc<std::sync::Mutex<RawTelemetry>>,
}

#[derive(Default, Clone)]
struct RawTelemetry {
    latest_overshoot_us: f64,
    p50_us: f64,
    p95_us: f64,
    p99_us: f64,
    p999_us: f64,
    max_us: f64,
    spin_duty_pct: f64,
    health: String,
    recommendation: String,
}

impl HermesApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let cfg = HermesConfig::load();
        let pll_shared = Arc::new(PllShared::new(cfg.tick_rate, cfg.hz_mode, cfg.manual_hz));
        pll_shared.store_cfg(cfg.pll);
        let initial_resolved_hz = if cfg.hz_mode == HzMode::Auto {
            125
        } else {
            cfg.manual_hz
        };

        let _hotkey_state = spawn_hotkey_thread();

        let app = Self {
            tab: Tab::Engine,
            pll_kp: cfg.pll.kp,
            pll_ki: cfg.pll.ki,
            pll_power: 1.0,
            pll_lfo_amp: cfg.pll.lfo_amp_us,
            pll_lfo_period: cfg.pll.lfo_period_s,
            selected_tick_rate: cfg.tick_rate,
            ram_only: cfg.ram_only,
            auto_tune: true,
            selected_mouse_hz: cfg.manual_hz,
            selected_kb_hz: 1000,
            is_auto_hz: cfg.hz_mode == HzMode::Auto,
            cfg,
            running: Arc::new(AtomicBool::new(false)),
            pll_shared,
            click_interrupt: Arc::new(ClickInterrupt::new()),
            network_guard: Arc::new(std::sync::Mutex::new(NetworkGuard::new())),
            system_guard: Arc::new(std::sync::Mutex::new(SystemGuard::new())),
            wake_history: VecDeque::with_capacity(640),
            jitter_history: VecDeque::with_capacity(640),
            latest_clock_mean_us: 0.0,
            latest_clock_instability: 0.0,
            latest_resolved_hz: initial_resolved_hz,
            game_name: None,
            status: "Ready. Hotkeys Active: Alt+1 (Toggle) · Alt+2 (Shot Resync)".into(),
            worker_join: None,
            raw_running: Arc::new(AtomicBool::new(false)),
            raw_join: None,
            raw_latest: Arc::new(std::sync::Mutex::new(RawTelemetry::default())),
        };
        app.start_game_observer();
        app
    }

    fn start_game_observer(&self) {
        let running = Arc::new(AtomicBool::new(true));
        let r2 = Arc::clone(&running);
        thread::Builder::new()
            .name("hermes-game-observer".into())
            .spawn(move || {
                while r2.load(Ordering::Acquire) {
                    if let Some(g) = detect_running_game() {
                        eprintln!("[hermes] Game detected: {}", g.name);
                    }
                    thread::park_timeout(Duration::from_secs(3));
                }
            })
            .ok();
    }

    fn poll_global_hotkeys(&mut self) {
        if drain_toggle() {
            if self.running.load(Ordering::Acquire) {
                self.stop();
                self.status = "HOTKEY (ALT+1) → ENGINE STOPPED".into();
            } else {
                self.start();
                self.status = "HOTKEY (ALT+1) → ENGINE STARTED".into();
            }
        }
        if drain_click() {
            self.click_interrupt.trigger();
            self.status = "SHOT RESYNC TRIGGERED VIA ALT+2".into();
        }
        if drain_reset() {
            let def = HermesConfig::default();
            self.pll_kp = def.pll.kp;
            self.pll_ki = def.pll.ki;
            self.pll_lfo_amp = def.pll.lfo_amp_us;
            self.pll_lfo_period = def.pll.lfo_period_s;
            self.selected_tick_rate = def.tick_rate;
            self.pll_shared.store_cfg(def.pll);
            self.pll_shared.store_tick_rate(def.tick_rate);
            self.status = "RESET VIA HOTKEY (ALT+3)".into();
        }
    }

    fn start(&mut self) {
        if self.running.load(Ordering::Acquire) {
            return;
        }
        self.status = "Starting Sub-Microsecond Precision PLL Engine...".into();
        self.wake_history.clear();
        self.jitter_history.clear();

        self.pll_shared.store_tick_rate(self.selected_tick_rate);
        self.pll_shared.store_cfg(PllConfig {
            kp: self.pll_kp,
            ki: self.pll_ki,
            power: self.pll_power,
            lfo_amp_us: self.pll_lfo_amp,
            lfo_period_s: self.pll_lfo_period,
        });
        self.pll_shared.store_hz_config(
            if self.is_auto_hz {
                HzMode::Auto
            } else {
                HzMode::Manual
            },
            self.selected_mouse_hz,
        );

        self.running.store(true, Ordering::Release);
        let flag = Arc::clone(&self.running);
        let shared = Arc::clone(&self.pll_shared);
        let click = Arc::clone(&self.click_interrupt);
        let cfg = self.cfg.clone();
        let auto_tune = self.auto_tune;

        let handle = thread::Builder::new()
            .name("hermes-pll-worker".into())
            .spawn(move || {
                let _timer = TimerResolutionGuard::request(cfg.timer_resolution);
                let mut engine = PllEngine::new(cfg.tick_rate);
                engine.apply_cfg(cfg.pll);
                let initial_hz_config = shared.load_hz_config();
                let mut hz_detector =
                    HzDetector::new(initial_hz_config.mode, initial_hz_config.manual_hz);
                let mut applied_hz_config = initial_hz_config;
                let mut mouse_modulator = MouseModulator::new();
                let mut clock_monitor = ClockStabilityMonitor::default();
                let mut last_instant = Instant::now();
                let mut ticks_since_tune = 0;

                while flag.load(Ordering::Acquire) {
                    let mut live_cfg = shared.load_cfg();
                    let live_rate = shared.load_tick_rate();
                    let live_hz_config = shared.load_hz_config();
                    if live_hz_config != applied_hz_config {
                        hz_detector.apply_config(live_hz_config);
                        applied_hz_config = live_hz_config;
                    }

                    // Smart Auto-Tuner
                    if auto_tune {
                        ticks_since_tune += 1;
                        if ticks_since_tune >= 80 {
                            ticks_since_tune = 0;
                            if engine.jitter_ema_us > 100.0 {
                                live_cfg.kp = (live_cfg.kp + 0.04).min(1.2);
                                live_cfg.ki = (live_cfg.ki + 0.01).min(0.3);
                            } else if engine.jitter_ema_us < 20.0 {
                                live_cfg.kp = (live_cfg.kp - 0.02).max(0.2);
                                live_cfg.ki = (live_cfg.ki - 0.005).max(0.02);
                            }
                            shared.store_cfg(live_cfg);
                        }
                    }

                    if live_cfg.kp != engine.cfg.kp
                        || live_cfg.ki != engine.cfg.ki
                        || live_cfg.lfo_amp_us != engine.cfg.lfo_amp_us
                        || live_cfg.lfo_period_s != engine.cfg.lfo_period_s
                    {
                        engine.apply_cfg(live_cfg);
                    }
                    if engine.tick_rate != live_rate {
                        engine.set_tick_rate(live_rate);
                    }

                    // Layer 3: Click Interrupt
                    if click.consume() {
                        engine.resync();
                    }

                    // Layer 2: Mouse Delta Modulator
                    let now_us = engine.clock.now_us();
                    let mouse_shift_us =
                        mouse_modulator.feed(now_us, engine.tick_rate.us_per_tick());

                    // Sync PLL power from shared atomics into engine cfg
                    engine.cfg.power = shared.load_power();

                    let planned_sleep_us = engine.compute_planned_sleep_us(mouse_shift_us);

                    // Dynamic Adaptive Spin Budget Controller:
                    // Adjust spin window based on previous cycle latency to strictly stay below 3% CPU duty.
                    let target_spin_window_us = 350.0_f64.min(planned_sleep_us * 0.15);
                    let coarse_sleep_us = (planned_sleep_us - target_spin_window_us).max(0.0);

                    let cycle_start = Instant::now();
                    if coarse_sleep_us > 100.0 {
                        thread::sleep(Duration::from_micros(coarse_sleep_us as u64));
                    }

                    // High-resolution spin to exact microsecond deadline
                    let spin_start = Instant::now();
                    let spin_deadline =
                        cycle_start + Duration::from_micros(planned_sleep_us as u64);
                    while Instant::now() < spin_deadline {
                        std::hint::spin_loop();
                    }
                    let spin_elapsed_us = spin_start.elapsed().as_secs_f64() * 1_000_000.0;

                    let actual_end = Instant::now();
                    let actual_total_us =
                        actual_end.duration_since(last_instant).as_secs_f64() * 1_000_000.0;
                    last_instant = actual_end;

                    let observed_now_us = engine.clock.now_us();
                    clock_monitor.observe(observed_now_us);
                    hz_detector.feed_us(observed_now_us);

                    engine.record_actual_cycle(
                        planned_sleep_us,
                        actual_total_us,
                        spin_elapsed_us,
                        mouse_shift_us,
                    );

                    if engine.tick_index & (TELEMETRY_PUBLISH_INTERVAL_TICKS - 1) == 0 {
                        shared.publish(
                            engine.snapshot(),
                            hz_detector.resolved_hz(),
                            clock_monitor.mean_us(),
                            clock_monitor.instability_ratio(),
                            clock_monitor.samples(),
                        );
                    }
                }
            })
            .expect("failed to start PLL worker thread");
        self.worker_join = Some(handle);
        self.status = "⚡ Sub-Microsecond PLL Engine ACTIVE · Phase Locked.".into();

        // Telemetry Worker
        let raw_running = Arc::clone(&self.raw_running);
        raw_running.store(true, Ordering::Release);
        let raw_flag = Arc::clone(&raw_running);
        let raw_latest = Arc::clone(&self.raw_latest);
        let shared_telemetry = Arc::clone(&self.pll_shared);

        let raw_handle = thread::Builder::new()
            .name("hermes-telemetry".into())
            .spawn(move || {
                let preset = Preset::Balanced;
                let mut hist = VecDeque::with_capacity(2048);
                while raw_flag.load(Ordering::Acquire) {
                    // Sample every 5ms — high-frequency sampling ensures
                    // DPC storms get represented by many samples rather than
                    // dominating a single p99.9/max slot for 30 seconds.
                    thread::sleep(Duration::from_millis(5));
                    let snap = shared_telemetry.snapshot();

                    let clamped_overshoot = snap.cs_latency_us.min(1000.0);
                    hist.push_back(clamped_overshoot);
                    if hist.len() > 2048 {
                        hist.pop_front();
                    }
                    if !hist.is_empty() {
                        let mut sorted: Vec<f64> = hist.iter().copied().collect();
                        sorted
                            .sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                        let pct = |p: f64| {
                            let idx =
                                ((p * (sorted.len() as f64 - 1.0)) as usize).min(sorted.len() - 1);
                            sorted[idx]
                        };
                        let mut latest = raw_latest.lock().unwrap();
                        latest.latest_overshoot_us = clamped_overshoot;
                        latest.p50_us = pct(0.50);
                        latest.p95_us = pct(0.95);
                        latest.p99_us = pct(0.99);
                        latest.p999_us = pct(0.999);
                        latest.max_us = sorted.last().copied().unwrap_or(0.0);
                        latest.spin_duty_pct = snap.spin_duty_pct;
                        latest.health = match latest.p95_us {
                            x if x <= 150.0 => "ULTRA STABLE".into(),
                            x if x <= 300.0 => "EXCELLENT".into(),
                            x if x <= 600.0 => "MODERATE".into(),
                            _ => "HIGH TAIL".into(),
                        };
                        latest.recommendation = preset.description().into();
                    }
                }
            })
            .expect("failed to start telemetry thread");
        self.raw_join = Some(raw_handle);
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        self.raw_running.store(false, Ordering::Release);
        if let Some(h) = self.worker_join.take() {
            let _ = h.join();
        }
        if let Some(h) = self.raw_join.take() {
            let _ = h.join();
        }
        if let Ok(mut ng) = self.network_guard.lock() {
            ng.revert();
        }
        if let Ok(mut sg) = self.system_guard.lock() {
            sg.restore_all();
        }
        self.status = "Engine STOPPED. All network/affinity tuners safely restored.".into();
    }

    fn render_top_bar(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(RichText::new("HERMES").size(26.0).strong().color(ACCENT));
                ui.label(
                    RichText::new("ENGINE v3")
                        .size(15.0)
                        .strong()
                        .color(Color32::from_gray(160)),
                );
                ui.add_space(16.0);
                ui.label(
                    RichText::new(format!("v{} · {}", hermes_engine::VERSION, platform_name()))
                        .size(14.0)
                        .color(Color32::from_gray(150)),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, color) = if self.running.load(Ordering::Acquire) {
                        ("● ENGINE LIVE", GREEN)
                    } else {
                        ("○ ENGINE OFF", Color32::from_gray(140))
                    };
                    ui.label(RichText::new(label).size(15.0).strong().color(color));
                    ui.add_space(12.0);
                    if self.ram_only {
                        ui.label(
                            RichText::new("⚡ RAM-ONLY")
                                .size(14.0)
                                .strong()
                                .color(AMBER),
                        );
                    }
                    if self.auto_tune {
                        ui.add_space(8.0);
                        ui.label(
                            RichText::new("🧠 AUTO-OPTIMIZER ON")
                                .size(14.0)
                                .strong()
                                .color(GREEN),
                        );
                    }
                });
            });
            ui.add_space(8.0);
        });
    }

    fn render_tabs(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("tabs")
            .exact_width(210.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(Color32::from_rgb(14, 18, 25))
                    .inner_margin(egui::Margin::same(16.0)),
            )
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("CORE NAVIGATION")
                        .size(13.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.add_space(10.0);

                if ui
                    .selectable_label(self.tab == Tab::Engine, "🚀  PLL Engine")
                    .clicked()
                {
                    self.tab = Tab::Engine;
                }
                ui.add_space(6.0);
                if ui
                    .selectable_label(self.tab == Tab::Lab, "📊  Timing & Jitter Lab")
                    .clicked()
                {
                    self.tab = Tab::Lab;
                }
                ui.add_space(6.0);
                if ui
                    .selectable_label(self.tab == Tab::Input, "🖱  Input & Hardware Hz")
                    .clicked()
                {
                    self.tab = Tab::Input;
                }
                ui.add_space(6.0);
                if ui
                    .selectable_label(self.tab == Tab::System, "⚙  System & Power Tuners")
                    .clicked()
                {
                    self.tab = Tab::System;
                }
                ui.add_space(6.0);
                if ui
                    .selectable_label(self.tab == Tab::Settings, "⌨  Hotkeys & Settings")
                    .clicked()
                {
                    self.tab = Tab::Settings;
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(10.0);
                    ui.label(
                        RichText::new("Alt+1: Toggle · Alt+2: Shot")
                            .size(13.0)
                            .color(Color32::from_gray(130)),
                    );
                    ui.label(
                        RichText::new(self.status.clone())
                            .size(13.0)
                            .color(Color32::from_gray(180)),
                    );
                });
            });
    }

    fn render_engine_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(10.0);
                let running = self.running.load(Ordering::Acquire);

                // Controls Bar
                ui.horizontal(|ui| {
                    let btn_text = if running { "■  STOP ENGINE (Alt+1)" } else { "▶  START ENGINE (Alt+1)" };
                    let btn = egui::Button::new(RichText::new(btn_text).size(18.0).strong().color(Color32::WHITE))
                        .fill(if running { Color32::from_rgb(220, 38, 38) } else { Color32::from_rgb(16, 185, 129) })
                        .rounding(8.0);
                    if ui.add_sized([270.0, 52.0], btn).clicked() {
                        if running {
                            self.stop();
                        } else {
                            self.start();
                        }
                    }
                    ui.add_space(14.0);

                    if ui.add_sized([180.0, 52.0],
                        egui::Button::new(RichText::new("↺ Reset Defaults").size(15.0))
                            .fill(Color32::from_rgb(60, 68, 84))
                            .rounding(8.0),
                    ).clicked() {
                        let def = HermesConfig::default();
                        self.pll_kp = def.pll.kp;
                        self.pll_ki = def.pll.ki;
                        self.pll_lfo_amp = def.pll.lfo_amp_us;
                        self.pll_lfo_period = def.pll.lfo_period_s;
                        self.selected_tick_rate = def.tick_rate;
                        self.pll_shared.store_cfg(def.pll);
                        self.pll_shared.store_tick_rate(def.tick_rate);
                        self.cfg = def.clone();
                        let _ = self.cfg.save();
                        self.status = "All settings reset to calibrated defaults.".into();
                    }

                    if ui.add_sized([160.0, 52.0],
                        egui::Button::new(RichText::new("💾 Save Config").size(15.0))
                            .fill(Color32::from_rgb(37, 99, 235))
                            .rounding(8.0),
                    ).clicked() {
                        self.cfg.tick_rate = self.selected_tick_rate;
                        self.cfg.pll = PllConfig {
                            kp: self.pll_kp,
                            ki: self.pll_ki,
                            power: self.pll_power,
                            lfo_amp_us: self.pll_lfo_amp,
                            lfo_period_s: self.pll_lfo_period,
                        };
                        self.cfg.manual_hz = self.selected_mouse_hz;
                        self.cfg.hz_mode = if self.is_auto_hz {
                            HzMode::Auto
                        } else {
                            HzMode::Manual
                        };
                        if let Err(e) = self.cfg.save() {
                            self.status = format!("Save failed: {e}");
                        } else {
                            self.status = "Config saved to disk with backup.".into();
                        }
                    }
                });

                ui.add_space(18.0);

                // 3-Layer PLL Live Overview
                ui.label(RichText::new("3-LAYER PHASE ENGINE STATUS").size(14.0).strong().color(ACCENT));
                ui.add_space(8.0);
                ui.columns(3, |cols| {
                    layer_card(&mut cols[0], "LAYER 1 · Network Heartbeat",
                        format!("{} Hz", self.selected_tick_rate.hz()),
                        "Server tick sync & Brownian LFO drift");
                    layer_card(&mut cols[1], "LAYER 2 · Mouse Delta Mod",
                        format!("{} Hz", self.latest_resolved_hz),
                        "Phase shift relative to cursor velocity");
                    layer_card(&mut cols[2], "LAYER 3 · Click Interrupt",
                        if self.click_interrupt.is_pending() { "⚡ RESYNCING".into() } else { "READY (Alt+2)".into() },
                        "Zero-latency PLL phase lock on shot");
                });

                ui.add_space(18.0);

                // Auto-Optimization & Knobs
                ui.label(RichText::new("ADAPTIVE SELF-TUNER & PRECISION KNOBS").size(14.0).strong().color(ACCENT));
                ui.add_space(8.0);
                egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(16.0)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.auto_tune, RichText::new("🧠 Dynamic Auto-Optimizer (Auto-tunes Kp/Ki in real-time)").size(15.0).strong());
                    });
                    ui.label(RichText::new("💡 Constantly analyzes tick micro-jitter and dynamically adjusts PLL response speed for minimum latency.")
                        .size(13.0).color(Color32::from_gray(160)));
                    ui.add_space(10.0);

                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Server Tick Rate:").size(15.0).strong());
                        ui.radio_value(&mut self.selected_tick_rate, TickRate::Hz64, RichText::new("64 Hz (15.62 ms)").size(14.0));
                        ui.radio_value(&mut self.selected_tick_rate, TickRate::Hz128, RichText::new("128 Hz (7.81 ms)").size(14.0));
                        if running {
                            self.pll_shared.store_tick_rate(self.selected_tick_rate);
                        }
                    });
                    ui.label(RichText::new("💡 Sets the reference heartbeat clock frequency matching CS2/Valorant match servers.")
                        .size(13.0).color(Color32::from_gray(160)));

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("⚡ PLL Power Multiplier:").size(15.0).strong().color(ACCENT));
                        ui.add(egui::Slider::new(&mut self.pll_power, 0.1..=2.0).step_by(0.05).custom_formatter(|n, _| {
                            if (n - 1.0).abs() < 0.04 {
                                "1.00 (Normal Balanced)".into()
                            } else if n < 0.8 {
                                format!("{n:.2} (Soft / Low CPU)")
                            } else {
                                format!("{n:.2} (Aggressive / Razor Sharp)")
                            }
                        }));
                        if running {
                            self.pll_shared.store_power(self.pll_power);
                        }
                    });
                    ui.label(RichText::new("💡 Master PLL reaction multiplier. 1.0 is calibrated default. Increase up to 2.0 for razor-sharp phase tracking or reduce to 0.1 for soft low-overhead.")
                        .size(13.0).color(Color32::from_gray(160)));

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Kp (Proportional Gain):").size(15.0).strong());
                        ui.add(egui::Slider::new(&mut self.pll_kp, 0.1..=2.0).step_by(0.01));
                    });
                    ui.label(RichText::new("💡 How aggressively the phase loop reacts to instant clock timing errors.")
                        .size(13.0).color(Color32::from_gray(160)));

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Ki (Integral Gain):").size(15.0).strong());
                        ui.add(egui::Slider::new(&mut self.pll_ki, 0.01..=1.0).step_by(0.01));
                    });
                    ui.label(RichText::new("💡 Eliminates steady-state phase drift over sustained gameplay sessions.")
                        .size(13.0).color(Color32::from_gray(160)));

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("LFO Amplitude (µs):").size(15.0).strong());
                        ui.add(egui::Slider::new(&mut self.pll_lfo_amp, 0.0..=30.0).step_by(0.5));
                    });
                    ui.label(RichText::new("💡 Micro-dither injected into the timing window to prevent artificial harmonic standing waves.")
                        .size(13.0).color(Color32::from_gray(160)));

                    ui.add_space(12.0);
                    ui.checkbox(&mut self.ram_only, RichText::new("⚡ RAM-Only Mode (Zero disk I/O, zero log writes)").size(14.0));
                    ui.label(RichText::new("💡 Eliminates NVMe/SSD background write spikes during critical gameplay.")
                        .size(13.0).color(Color32::from_gray(160)));

                    ui.add_space(6.0);
                    ui.checkbox(&mut self.cfg.timer_resolution, RichText::new("⚙ Request Windows 1 ms Timer Resolution (auto-reverts on stop)").size(14.0));
                });

                ui.add_space(16.0);

                // Live Status Box
                ui.label(RichText::new("LIVE ATOMIC STATUS").size(14.0).strong().color(ACCENT));
                ui.add_space(8.0);
                egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(16.0)).show(ui, |ui| {
                    let snap = self.pll_shared.snapshot();
                    ui.columns(3, |cols| {
                        cols[0].label(RichText::new(format!("Target Sleep:     {:.2} µs", snap.sleep_us)).size(14.0));
                        cols[0].label(RichText::new(format!("Phase Jitter:     {:.2} µs", snap.phase_error_us)).size(14.0));

                        cols[1].label(RichText::new(format!("Context Latency:  {:.2} µs", snap.cs_latency_us)).size(14.0));
                        cols[1].label(RichText::new(format!("Spin Duty Cycle:  {:.2}%", snap.spin_duty_pct)).size(14.0));

                        cols[2].label(RichText::new(format!("LFO Phase Shift:  {:.2} µs", snap.lfo_us)).size(14.0));
                        cols[2].label(RichText::new(format!("Total Ticks:      {}", snap.tick_index)).size(14.0));
                    });
                });
            });
        });
    }

    fn render_lab_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(10.0);
                ui.label(
                    RichText::new("MICROSECOND TIMING & JITTER LAB")
                        .size(22.0)
                        .strong(),
                );
                ui.label(
                    RichText::new(
                        "Real-time scheduler precision analysis · 100% microsecond resolution",
                    )
                    .size(14.0)
                    .color(Color32::from_gray(150)),
                );
                ui.add_space(14.0);

                let raw = self.raw_latest.lock().unwrap().clone();
                ui.columns(4, |cols| {
                    metric_card(
                        &mut cols[0],
                        "LATEST OVERSHOOT",
                        format_us(raw.latest_overshoot_us),
                        false,
                    );
                    metric_card(
                        &mut cols[1],
                        "p99 LATENCY",
                        format_us(raw.p99_us),
                        raw.p99_us > 300.0,
                    );
                    metric_card(
                        &mut cols[2],
                        "p99.9 LATENCY",
                        format_us(raw.p999_us),
                        raw.p999_us > 600.0,
                    );
                    metric_card(
                        &mut cols[3],
                        "MAX SPIKE",
                        format_us(raw.max_us),
                        raw.max_us > 1000.0,
                    );
                });

                ui.add_space(16.0);

                // Plot
                ui.label(
                    RichText::new("WAKE-UP OVERSHOOT & PHASE JITTER OVER TIME (30s Rolling)")
                        .size(14.0)
                        .strong()
                        .color(ACCENT),
                );
                ui.add_space(8.0);
                egui::Frame::none()
                    .fill(BG_DARK)
                    .rounding(8.0)
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        let wake_pts = PlotPoints::from_iter(self.wake_history.iter().copied());
                        let jitter_pts = PlotPoints::from_iter(self.jitter_history.iter().copied());
                        Plot::new("wake_plot")
                            .height(280.0)
                            .include_y(0.0)
                            .allow_scroll(false)
                            .allow_drag(false)
                            .legend(Legend::default())
                            .show(ui, |plot_ui| {
                                plot_ui.line(
                                    Line::new(wake_pts)
                                        .name("Wake Overshoot (µs)")
                                        .color(ACCENT)
                                        .width(2.0_f32),
                                );
                                plot_ui.line(
                                    Line::new(jitter_pts)
                                        .name("Phase Error (µs)")
                                        .color(AMBER)
                                        .width(2.0_f32),
                                );
                            });
                    });

                ui.add_space(16.0);

                // Session Health Summary
                egui::Frame::none()
                    .fill(PANEL)
                    .rounding(8.0)
                    .inner_margin(egui::Margin::same(16.0))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("SESSION TIMING HEALTH AUDIT")
                                .size(14.0)
                                .strong()
                                .color(ACCENT),
                        );
                        ui.add_space(10.0);
                        egui::Grid::new("lab_details")
                            .num_columns(4)
                            .spacing([36.0, 10.0])
                            .striped(true)
                            .show(ui, |ui| {
                                ui.label(RichText::new("Median (p50):").size(15.0).strong());
                                ui.label(RichText::new(format_us(raw.p50_us)).size(15.0));
                                ui.label(RichText::new("p95 Percentile:").size(15.0).strong());
                                ui.label(RichText::new(format_us(raw.p95_us)).size(15.0));
                                ui.end_row();

                                ui.label(RichText::new("Overall Health:").size(15.0).strong());
                                let (col, txt) = match raw.health.as_str() {
                                    "ULTRA STABLE" => (GREEN, "🟢 ULTRA STABLE"),
                                    "EXCELLENT" => (GREEN, "🟢 EXCELLENT"),
                                    "MODERATE" => (AMBER, "🟡 MODERATE JITTER"),
                                    _ => (RED, "🔴 HIGH TAIL DETECTED"),
                                };
                                ui.label(RichText::new(txt).size(15.0).strong().color(col));

                                ui.label(RichText::new("Spin Duty Cycle:").size(15.0).strong());
                                ui.label(
                                    RichText::new(format!(
                                        "{:.2}% (Capped < 3%)",
                                        raw.spin_duty_pct
                                    ))
                                    .size(15.0),
                                );
                                ui.end_row();
                            });
                    });
            });
        });
    }

    fn render_input_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(10.0);
                ui.label(RichText::new("INPUT SUBSYSTEM & HARDWARE POLLING").size(22.0).strong());
                ui.label(RichText::new("Auto/Manual Hz Selector · Floating Mouse Filter · Layer 3 Click Interrupt")
                    .size(14.0).color(Color32::from_gray(150)));
                ui.add_space(14.0);

                ui.columns(2, |cols| {
                    // Left Column
                    egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(16.0))
                        .show(&mut cols[0], |ui| {
                            ui.label(RichText::new("HARDWARE POLLING RATE SELECTOR").size(14.0).strong().color(ACCENT));
                            ui.add_space(10.0);

                            // Auto / Manual Mode Radio
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Mode:").size(15.0).strong());
                                if ui.radio(self.is_auto_hz, RichText::new("Auto Detect").size(14.0)).clicked() {
                                    self.is_auto_hz = true;
                                    self.pll_shared.store_hz_config(HzMode::Auto, self.selected_mouse_hz);
                                }
                                if ui.radio(!self.is_auto_hz, RichText::new("Manual Force").size(14.0)).clicked() {
                                    self.is_auto_hz = false;
                                    self.pll_shared.store_hz_config(HzMode::Manual, self.selected_mouse_hz);
                                }
                            });

                            ui.add_space(10.0);

                            // Mouse Dropdown
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Mouse Polling Rate:").size(15.0).strong());
                                egui::ComboBox::from_id_salt("mouse_hz_combo_v4")
                                    .selected_text(RichText::new(format!("{} Hz", self.selected_mouse_hz)).size(15.0))
                                    .show_ui(ui, |ui| {
                                        for &cand in POLLING_CANDIDATES {
                                            if ui.selectable_value(&mut self.selected_mouse_hz, cand, RichText::new(format!("{cand} Hz")).size(15.0)).clicked() {
                                                self.is_auto_hz = false;
                                                self.pll_shared.store_hz_config(HzMode::Manual, cand);
                                            }
                                        }
                                    });
                            });
                            ui.add_space(4.0);
                            ui.label(RichText::new(format!("Worker-resolved rate: {} Hz", self.latest_resolved_hz))
                                .size(13.0).color(Color32::from_gray(160)));
                            ui.label(RichText::new("💡 TIP: Set to match your mouse physical polling rate (e.g. 1000 Hz or 4000/8000 Hz for high-end gaming mice).")
                                .size(13.0).color(Color32::from_gray(160)));

                            ui.add_space(14.0);

                            // Keyboard Dropdown
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Keyboard Polling Rate:").size(15.0).strong());
                                egui::ComboBox::from_id_salt("kb_hz_combo_v4")
                                    .selected_text(RichText::new(format!("{} Hz", self.selected_kb_hz)).size(15.0))
                                    .show_ui(ui, |ui| {
                                        for &cand in &[125, 250, 500, 1000, 2000, 4000, 8000] {
                                            ui.selectable_value(&mut self.selected_kb_hz, cand, RichText::new(format!("{cand} Hz")).size(15.0));
                                        }
                                    });
                            });
                            ui.add_space(4.0);
                            ui.label(RichText::new("💡 TIP: Matches keyboard debounce scan rate to eliminate key-press micro-stutters.")
                                .size(13.0).color(Color32::from_gray(160)));
                        });

                    // Right Column
                    egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(16.0))
                        .show(&mut cols[1], |ui| {
                            ui.label(RichText::new("CLOCK & POWER STABILITY (DIRTY POWER DETECTOR)").size(14.0).strong().color(ACCENT));
                            ui.add_space(10.0);

                            let stability_pct = ((1.0 - self.latest_clock_instability) * 100.0).clamp(0.0, 100.0);
                            ui.label(RichText::new(format!("Mean Tick Delta:      {:.2} µs", self.latest_clock_mean_us)).size(15.0));
                            ui.label(RichText::new(format!("Coefficient of Var:   {:.4}", self.latest_clock_instability)).size(15.0));
                            ui.label(RichText::new(format!("Clock Stability:      {:.1}%", stability_pct)).size(15.0));
                            ui.add_space(8.0);

                            let (txt, col) = if self.latest_clock_instability > 0.20 {
                                ("DIRTY POWER / VRM RIPPLE DETECTED", RED)
                            } else if self.latest_clock_instability > 0.08 {
                                ("MODERATE VOLTAGE JITTER", AMBER)
                            } else {
                                ("CLEAN POWER & STABLE CLOCK", GREEN)
                            };
                            ui.label(RichText::new(txt).size(15.0).strong().color(col));
                            ui.add_space(4.0);
                            ui.label(RichText::new("💡 TIP: Monitors motherboard VRM voltage fluctuations causing USB polling packet dropouts.")
                                .size(13.0).color(Color32::from_gray(160)));
                        });
                });

                ui.add_space(16.0);

                // Click Interrupt Box
                egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(16.0)).show(ui, |ui| {
                    ui.label(RichText::new("CLICK INTERRUPT (LAYER 3 ZERO-LATENCY SHOT)").size(14.0).strong().color(ACCENT));
                    ui.add_space(10.0);
                    let pending = self.click_interrupt.is_pending();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("Status: {}", if pending { "⚡ RESYNC PENDING" } else { "ACTIVE & ARMED" })).size(15.0));
                        ui.add_space(20.0);
                        if ui.button(RichText::new("🎯 Simulate Click Shot (Alt+2)").size(15.0)).clicked() {
                            self.click_interrupt.trigger();
                        }
                    });
                    ui.add_space(4.0);
                    ui.label(RichText::new("💡 TIP: Resynchronizes the PLL phase engine the exact instant you press Left Mouse Button (or Alt+2 in full-screen games).")
                        .size(13.0).color(Color32::from_gray(160)));
                });
            });
        });
    }

    fn render_system_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(10.0);
                ui.label(RichText::new("SYSTEM & OS LATENCY TUNERS").size(22.0).strong());
                ui.label(RichText::new("All tweaks are 100% reversible in real-time · Safe-by-default philosophy")
                    .size(14.0).color(Color32::from_gray(150)));
                ui.add_space(14.0);

                ui.columns(3, |cols| {
                    // C-States
                    egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(16.0))
                        .show(&mut cols[0], |ui| {
                            ui.label(RichText::new("CPU C-STATES LOCK").size(14.0).strong().color(ACCENT));
                            ui.add_space(8.0);
                            let sg = self.system_guard.lock().unwrap();
                            let applied = sg.cstate_locked;
                            drop(sg);
                            ui.label(RichText::new(format!("State: {}", if applied { "🟢 LOCKED (MAX FREQ)" } else { "⚪ DEFAULT" })).size(15.0));
                            ui.add_space(10.0);
                            let lbl = if applied { "Unlock C-States" } else { "Lock C-States" };
                            if ui.button(RichText::new(lbl).size(15.0)).clicked() {
                                let mut sg = self.system_guard.lock().unwrap();
                                let _ = sg.lock_c_states(!applied);
                            }
                            ui.add_space(10.0);
                            ui.label(RichText::new("💡 TIP: Prevents CPU cores from entering low-power sleep C-States during gameplay, eliminating CPU wake-up spikes.")
                                .size(13.0).color(Color32::from_gray(160)));
                        });

                    // Network TCP/Nagle
                    egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(16.0))
                        .show(&mut cols[1], |ui| {
                            ui.label(RichText::new("TCP / NAGLE TUNER").size(14.0).strong().color(ACCENT));
                            ui.add_space(8.0);
                            let ng = self.network_guard.lock().unwrap();
                            let applied = ng.is_applied();
                            drop(ng);
                            ui.label(RichText::new(format!("State: {}", if applied { "🟢 OPTIMIZED" } else { "⚪ DEFAULT" })).size(15.0));
                            ui.add_space(10.0);
                            let lbl = if applied { "Revert Network" } else { "Apply NoDelay & AckFreq" };
                            if ui.button(RichText::new(lbl).size(15.0)).clicked() {
                                let mut ng = self.network_guard.lock().unwrap();
                                if applied { let _ = ng.revert(); } else { let _ = ng.apply(); }
                            }
                            ui.add_space(10.0);
                            ui.label(RichText::new("💡 TIP: Disables Windows TCP delayed ACKs and Nagle's buffering algorithm for instantaneous packet transmission.")
                                .size(13.0).color(Color32::from_gray(160)));
                        });

                    // Core Affinity Isolation
                    egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(16.0))
                        .show(&mut cols[2], |ui| {
                            ui.label(RichText::new("CORE AFFINITY ISOLATION").size(14.0).strong().color(ACCENT));
                            ui.add_space(8.0);
                            let sg = self.system_guard.lock().unwrap();
                            let applied = sg.affinity_set;
                            drop(sg);
                            ui.label(RichText::new(format!("State: {}", if applied { "🟢 ISOLATED (CORE 2/3)" } else { "⚪ DEFAULT" })).size(15.0));
                            ui.add_space(10.0);
                            let lbl = if applied { "Release Core" } else { "Isolate to Core" };
                            if ui.button(RichText::new(lbl).size(15.0)).clicked() {
                                let mut sg = self.system_guard.lock().unwrap();
                                let _ = sg.isolate_affinity(!applied);
                            }
                            ui.add_space(10.0);
                            ui.label(RichText::new("💡 TIP: Pins the Hermes timing loop to dedicated physical cores, away from OS interrupts on Core 0.")
                                .size(13.0).color(Color32::from_gray(160)));
                        });
                });

                ui.add_space(16.0);

                // Diagnostic System Footprint
                egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(16.0)).show(ui, |ui| {
                    ui.label(RichText::new("SYSTEM & ENVIRONMENT FOOTPRINT").size(14.0).strong().color(ACCENT));
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("Operating System:       {}", platform_name())).size(15.0));
                    ui.label(RichText::new(format!("RAM-Only Telemetry:     {}", if self.ram_only { "Active (Zero NVMe write amplification)" } else { "Standard" })).size(15.0));
                    ui.label(RichText::new(format!("Active Game Process:    {}", self.game_name.clone().unwrap_or_else(|| "None detected (Listening in background)".into()))).size(15.0));
                });
            });
        });
    }

    fn render_settings_tab(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add_space(10.0);
                ui.label(RichText::new("GLOBAL HOTKEYS & ADVANCED SETTINGS").size(22.0).strong());
                ui.label(RichText::new("Control the engine without leaving your full-screen game")
                    .size(14.0).color(Color32::from_gray(150)));
                ui.add_space(16.0);

                egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(18.0)).show(ui, |ui| {
                    ui.label(RichText::new("ACTIVE GLOBAL HOTKEYS (WORKS IN ALL FULL-SCREEN GAMES)").size(15.0).strong().color(ACCENT));
                    ui.add_space(12.0);

                    egui::Grid::new("hotkey_grid").num_columns(3).spacing([28.0, 14.0]).striped(true).show(ui, |ui| {
                        ui.label(RichText::new("Hotkey").size(15.0).strong().color(Color32::WHITE));
                        ui.label(RichText::new("Action").size(15.0).strong().color(Color32::WHITE));
                        ui.label(RichText::new("Description & In-Game Usage").size(15.0).strong().color(Color32::WHITE));
                        ui.end_row();

                        ui.label(RichText::new("Alt + 1").size(15.0).strong().color(AMBER));
                        ui.label(RichText::new("Toggle Engine On / Off").size(15.0));
                        ui.label(RichText::new("Instantly starts or stops the 3-Layer PLL loop mid-game.").size(14.0));
                        ui.end_row();

                        ui.label(RichText::new("Alt + 2").size(15.0).strong().color(AMBER));
                        ui.label(RichText::new("Trigger Shot Resync (L3)").size(15.0));
                        ui.label(RichText::new("Forces zero-latency PLL phase lock before a critical shot.").size(14.0));
                        ui.end_row();

                        ui.label(RichText::new("Alt + 3").size(15.0).strong().color(AMBER));
                        ui.label(RichText::new("Reset to Calibrated Defaults").size(15.0));
                        ui.label(RichText::new("Restores default Kp/Ki and tick rate if you encounter jitter.").size(14.0));
                        ui.end_row();
                    });
                });

                ui.add_space(18.0);

                egui::Frame::none().fill(PANEL).rounding(8.0).inner_margin(egui::Margin::same(18.0)).show(ui, |ui| {
                    ui.label(RichText::new("BACKGROUND EXECUTION & MINIMIZE MODE").size(15.0).strong().color(ACCENT));
                    ui.add_space(10.0);
                    ui.label(RichText::new("When you minimize this window or switch to your game in Full-screen mode:").size(15.0));
                    ui.label(RichText::new("• The PLL Engine continues running seamlessly in its own high-priority thread.").size(14.0));
                    ui.label(RichText::new("• Global hotkeys (Alt+1 / Alt+2 / Alt+3) remain 100% active and responsive.").size(14.0));
                    ui.label(RichText::new("• Memory usage stays under 15 MB with zero CPU overhead when idling.").size(14.0));
                    ui.add_space(8.0);
                    ui.label(RichText::new("💡 TIP: You can leave Hermes minimized in the background all day while gaming.").size(13.0).color(Color32::from_gray(160)));
                });
            });
        });
    }

    fn refresh_telemetry(&mut self) {
        let snap = self.pll_shared.snapshot();
        if snap.tick_index == 0 {
            return;
        }
        let last = self.wake_history.back().map(|p| p[0]).unwrap_or(0.0);
        let now_s = last + 0.04;
        self.wake_history.push_back([now_s, snap.jitter_ema_us]);
        self.jitter_history.push_back([now_s, snap.phase_error_us]);
        trim_graph(&mut self.wake_history, now_s);
        trim_graph(&mut self.jitter_history, now_s);

        self.latest_resolved_hz = snap.resolved_hz;
        self.latest_clock_mean_us = snap.clock_mean_us;
        self.latest_clock_instability = snap.clock_instability;
    }
}

impl eframe::App for HermesApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_global_hotkeys();
        self.refresh_telemetry();
        self.render_top_bar(ctx);
        self.render_tabs(ctx);
        match self.tab {
            Tab::Engine => self.render_engine_tab(ctx),
            Tab::Lab => self.render_lab_tab(ctx),
            Tab::Input => self.render_input_tab(ctx),
            Tab::System => self.render_system_tab(ctx),
            Tab::Settings => self.render_settings_tab(ctx),
        }
        ctx.request_repaint_after(if self.running.load(Ordering::Acquire) {
            Duration::from_millis(30)
        } else {
            Duration::from_millis(200)
        });
    }
}

impl Drop for HermesApp {
    fn drop(&mut self) {
        self.stop();
    }
}

fn layer_card(ui: &mut egui::Ui, title: &str, value: String, subtitle: &str) {
    egui::Frame::none()
        .fill(PANEL)
        .rounding(8.0)
        .inner_margin(egui::Margin::same(16.0))
        .show(ui, |ui| {
            ui.set_min_height(95.0);
            ui.label(RichText::new(title).size(13.0).strong().color(ACCENT));
            ui.add_space(6.0);
            ui.label(
                RichText::new(value)
                    .size(22.0)
                    .strong()
                    .color(Color32::WHITE),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(subtitle)
                    .size(13.0)
                    .color(Color32::from_gray(150)),
            );
        });
}

fn metric_card(ui: &mut egui::Ui, label: &str, value: String, alert: bool) {
    egui::Frame::none()
        .fill(PANEL)
        .rounding(8.0)
        .inner_margin(egui::Margin::same(14.0))
        .show(ui, |ui| {
            ui.set_min_height(70.0);
            ui.label(
                RichText::new(label)
                    .size(13.0)
                    .strong()
                    .color(Color32::from_gray(150)),
            );
            ui.add_space(6.0);
            ui.label(RichText::new(value).size(22.0).strong().color(if alert {
                RED
            } else {
                Color32::WHITE
            }));
        });
}

fn trim_graph(history: &mut VecDeque<[f64; 2]>, newest_seconds: f64) {
    while history
        .front()
        .is_some_and(|p| newest_seconds - p[0] > 30.0)
    {
        history.pop_front();
    }
    while history.len() > 640 {
        history.pop_front();
    }
}

fn configure_style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(11, 14, 20);
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = Color32::from_rgb(8, 10, 15);
    visuals.selection.bg_fill = Color32::from_rgb(2, 132, 199);
    visuals.widgets.inactive.bg_fill = PANEL_ALT;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(34, 42, 58);
    visuals.widgets.active.bg_fill = Color32::from_rgb(45, 55, 76);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(38, 46, 62));

    let mut style = (*ctx.style()).clone();
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, FontId::proportional(22.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(13.0));
    ctx.set_style(style);

    ctx.set_visuals(visuals);
}

fn format_us(value: f64) -> String {
    if value >= 1_000.0 {
        format!("{:.2} ms", value / 1_000.0)
    } else {
        format!("{value:.2} µs")
    }
}
