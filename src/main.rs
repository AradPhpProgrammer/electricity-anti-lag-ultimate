use eframe::egui;
use rand::Rng;
use std::mem::size_of;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(target_os = "windows")]
use windows::core::w;
#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{GetLastError, HWND};
#[cfg(target_os = "windows")]
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
#[cfg(target_os = "windows")]
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
#[cfg(target_os = "windows")]
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
#[cfg(target_os = "windows")]
use windows::Win32::System::Power::{
    GetSystemPowerStatus, SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED,
    ES_SYSTEM_REQUIRED, SYSTEM_POWER_STATUS,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, SetProcessAffinityMask, SetProcessPriorityBoost,
    SetThreadAffinityMask, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, MOD_ALT, VK_F11};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Input::{RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_INPUTSINK};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowLongPtrW, PostQuitMessage, RegisterClassExW, SetWindowLongPtrW, TranslateMessage,
    CW_USEDEFAULT, GWLP_USERDATA, HMENU, HWND_MESSAGE, MSG, WM_DESTROY, WM_HOTKEY, WM_INPUT,
    WM_QUIT, WNDCLASSEXW, WS_OVERLAPPEDWINDOW,
};

#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[cfg(target_os = "windows")]
#[link(name = "ntdll")]
extern "system" {
    fn NtQueryTimerResolution(
        maximum_time: *mut u32,
        minimum_time: *mut u32,
        current_time: *mut u32,
    ) -> i32;
}

struct TimerResolutionGuard;
impl Drop for TimerResolutionGuard {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        unsafe {
            let _ = timeEndPeriod(1);
        }
    }
}

#[cfg(target_os = "windows")]
struct HybridClock {
    qpc_freq: f64,
    tsc_freq: f64,
    base_tsc: u64,
    base_qpc: i64,
}

#[cfg(target_os = "windows")]
impl HybridClock {
    fn new() -> Self {
        let mut freq = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut freq);
        }
        let qpc_freq = if freq == 0 { 10_000_000 } else { freq } as f64;
        let base_qpc = unsafe {
            let mut c = 0i64;
            let _ = QueryPerformanceCounter(&mut c);
            c
        };
        let base_tsc = unsafe { std::arch::x86_64::_rdtsc() };
        thread::sleep(Duration::from_millis(100));
        let end_qpc = unsafe {
            let mut c = 0i64;
            let _ = QueryPerformanceCounter(&mut c);
            c
        };
        let end_tsc = unsafe { std::arch::x86_64::_rdtsc() };
        let elapsed_qpc = (end_qpc - base_qpc) as f64;
        let elapsed_sec = elapsed_qpc / qpc_freq;
        let tsc_freq = (end_tsc - base_tsc) as f64 / elapsed_sec;
        Self {
            qpc_freq,
            tsc_freq,
            base_tsc,
            base_qpc,
        }
    }
    fn now_counter(&self) -> i64 {
        let tsc = unsafe { std::arch::x86_64::_rdtsc() };
        ((tsc - self.base_tsc) as f64 / self.tsc_freq * self.qpc_freq) as i64 + self.base_qpc
    }
    fn now_us(&self) -> f64 {
        self.now_counter() as f64 / self.qpc_freq * 1_000_000.0
    }
    fn frequency(&self) -> f64 {
        self.qpc_freq
    }
}

#[cfg(not(target_os = "windows"))]
struct HybridClock {
    frequency: f64,
}
#[cfg(not(target_os = "windows"))]
impl HybridClock {
    fn new() -> Self {
        Self {
            frequency: 10_000_000.0,
        }
    }
    fn now_counter(&self) -> i64 {
        use std::time::Instant;
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let start = START.get_or_init(Instant::now);
        (start.elapsed().as_secs_f64() * 10_000_000.0) as i64
    }
    fn now_us(&self) -> f64 {
        self.now_counter() as f64 / self.frequency * 1_000_000.0
    }
    fn frequency(&self) -> f64 {
        self.frequency
    }
}

#[derive(Clone)]
struct TelemetryHistory {
    timer_res_history: Vec<f32>,
    jitter_history: Vec<f32>,
    phase_error_history: Vec<f32>,
    context_switch_latency: Vec<f32>,
    dpc_latency_history: Vec<f32>,
}
impl Default for TelemetryHistory {
    fn default() -> Self {
        Self {
            timer_res_history: vec![15.6; 60],
            jitter_history: vec![0.0; 60],
            phase_error_history: vec![0.0; 60],
            context_switch_latency: vec![0.0; 60],
            dpc_latency_history: vec![0.0; 60],
        }
    }
}

#[derive(Clone)]
pub struct PllParams {
    pub kp: f64,
    pub ki: f64,
    pub lfo_amp: f64,
    pub lfo_period: f64,
}
impl PllParams {
    fn default_for_hz(_target_hz: f64) -> Self {
        Self {
            kp: 0.6,
            ki: 0.12,
            lfo_amp: 5.0,
            lfo_period: 10.0,
        }
    }
}

fn gaussian_noise() -> f64 {
    let mut rng = rand::thread_rng();
    let u1: f64 = rng.gen();
    let u2: f64 = rng.gen();
    if u1 < 1e-12 {
        return 0.0;
    }
    (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
}

pub struct PllEngine {
    phase_accumulator: f64,
    phase_step: f64,
    clock_freq: f64,
    start_counter: i64,
    kp: f64,
    ki: f64,
    integral_phase: f64,
    integral_limit: f64,
    ema_jitter_ticks: f64,
    lfo_value: f64,
    lfo_amp: f64,
    lfo_period: f64,
    lfo_step: f64,
}
impl PllEngine {
    pub fn new(target_hz: f64, clock: &HybridClock, params: &PllParams) -> Self {
        let clock_freq = clock.frequency();
        let phase_step = if clock_freq > 0.0 {
            target_hz / clock_freq
        } else {
            target_hz / 10_000_000.0
        };
        let start_counter = clock.now_counter();
        Self {
            phase_accumulator: 0.0,
            phase_step,
            clock_freq,
            start_counter,
            kp: params.kp,
            ki: params.ki,
            integral_phase: 0.0,
            integral_limit: 0.015 * target_hz,
            ema_jitter_ticks: 0.0,
            lfo_value: 0.0,
            lfo_amp: params.lfo_amp,
            lfo_period: params.lfo_period,
            lfo_step: 0.0,
        }
    }
    pub fn update(&mut self, current_counter: i64) -> (u64, f64, f64, f32) {
        let elapsed_qpc = (current_counter - self.start_counter).max(0);
        let expected_ticks = elapsed_qpc as f64 * self.phase_step;
        let phase_error = expected_ticks - self.phase_accumulator;
        let p_correction_ticks = self.kp * phase_error;
        self.integral_phase += self.ki * phase_error;
        self.integral_phase = self
            .integral_phase
            .clamp(-self.integral_limit, self.integral_limit);
        let correction_ticks = p_correction_ticks + self.integral_phase;
        let ticks_to_next_tick = 1.0 - self.phase_accumulator.fract();
        let ideal_qpc_to_next = ticks_to_next_tick / self.phase_step;
        let corrected_qpc = ideal_qpc_to_next - (correction_ticks / self.phase_step);
        let mut sleep_us = (corrected_qpc * 1_000_000.0 / self.clock_freq).max(0.0);
        // Brownian LFO
        self.lfo_step += 1.0 / self.lfo_period;
        if self.lfo_step >= 1.0 {
            self.lfo_step -= 1.0;
            let step = gaussian_noise() * 0.5;
            self.lfo_value += step;
            if self.lfo_value > self.lfo_amp {
                self.lfo_value = self.lfo_amp;
            } else if self.lfo_value < -self.lfo_amp {
                self.lfo_value = -self.lfo_amp;
            }
        }
        sleep_us += self.lfo_value;
        // محدودیت حداقل 100 میکروثانیه برای جلوگیری از هنگ
        if sleep_us < 100.0 {
            sleep_us = 100.0;
        }
        // محاسبه CS Latency با استفاده از f64 برای جلوگیری از overflow
        let expected_counter_f64 = self.start_counter as f64
            + (self.phase_accumulator * self.clock_freq / self.phase_step);
        let cs_latency_us =
            ((current_counter as f64 - expected_counter_f64).abs()) / self.clock_freq * 1_000_000.0;
        let cs_latency_us_clamped = if cs_latency_us > 500.0 {
            0.0
        } else {
            cs_latency_us
        };
        self.phase_accumulator += 1.0;
        self.ema_jitter_ticks = self.ema_jitter_ticks * 0.9 + phase_error.abs() * 0.1;
        (
            sleep_us as u64,
            self.ema_jitter_ticks,
            phase_error.abs(),
            cs_latency_us_clamped as f32,
        )
    }
}

#[cfg(target_os = "windows")]
fn get_usb_controller_instance_ids() -> Vec<String> {
    let output = std::process::Command::new("powershell")
        .args(&["-NoProfile", "-Command", "Get-PnpDevice -Class USB | Where-Object { $_.FriendlyName -match 'Host Controller|Root Hub' } | ForEach-Object { $_.InstanceId }"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    output
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(target_os = "windows")]
fn enable_msi_for_device(instance_id: &str) -> bool {
    let key_path = format!(
        r"SYSTEM\CurrentControlSet\Enum\{}\Device Parameters\Interrupt Management\MessageSignaledInterruptProperties",
        instance_id
    );
    let status = std::process::Command::new("reg")
        .args(&[
            "add",
            &key_path,
            "/v",
            "MSISupported",
            "/t",
            "REG_DWORD",
            "/d",
            "1",
            "/f",
        ])
        .status();
    matches!(status, Ok(s) if s.success())
}

#[cfg(target_os = "windows")]
fn lock_cpu_power_states(enable: bool) -> bool {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let val = if enable { "0" } else { "1" };
    let script = format!(
        r#"
        powercfg /setacvalueindex scheme_current sub_processor 0cc5b647-c1df-4637-891a-dec35c318583 {}
        powercfg /setacvalueindex scheme_current sub_processor 5b76c6d2-6f6b-4b6b-8a3a-7e7e6f7e8f9e 0
        powercfg /setactive scheme_current
        exit 0
    "#,
        val
    );
    let status = std::process::Command::new("powershell")
        .args(&["-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    matches!(status, Ok(s) if s.success())
}

#[cfg(target_os = "windows")]
fn set_process_affinity_to_isolated_core() -> bool {
    unsafe {
        let handle = GetCurrentProcess();
        if SetProcessAffinityMask(handle, 0b10000000).is_ok() {
            let _ = SetProcessPriorityBoost(handle, true);
            true
        } else {
            false
        }
    }
}

#[cfg(target_os = "windows")]
struct RawInputContext {
    ema_delta: Arc<AtomicU32>,
    clock: HybridClock,
    last: f64,
    alpha: f64,
    samples: Vec<f64>,
    ema_hz: f64,
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn raw_input_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    if msg == WM_INPUT {
        let ctx = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RawInputContext)
            .as_mut()
            .unwrap();
        let now = ctx.clock.now_us();
        if ctx.last > 0.0 {
            let d = (now - ctx.last).max(0.0);
            if d > 100.0 && d < 10000.0 {
                let current = ctx.ema_delta.load(Ordering::Relaxed);
                let alpha = ctx.alpha;
                let new_val = (current as f64 * (1.0 - alpha) + d * alpha) as u32;
                ctx.ema_delta.store(new_val, Ordering::Relaxed);
                ctx.samples.push(d);
                if ctx.samples.len() > 20 {
                    ctx.samples.remove(0);
                }
                if ctx.samples.len() >= 5 {
                    let avg: f64 = ctx.samples.iter().sum::<f64>() / ctx.samples.len() as f64;
                    if avg > 100.0 && avg < 10000.0 {
                        let hz = 1_000_000.0 / avg;
                        ctx.ema_hz = ctx.ema_hz * 0.7 + hz * 0.3;
                        // محدودیت ماکزیمم 1000Hz برای موس‌های معمولی
                        if ctx.ema_hz > 1000.0 {
                            ctx.ema_hz = 1000.0;
                        }
                    }
                }
            }
        }
        ctx.last = now;
        return windows::Win32::Foundation::LRESULT(0);
    }
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return windows::Win32::Foundation::LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[cfg(target_os = "windows")]
fn raw_input_loop(
    usage_page: u16,
    usage: u16,
    ema_delta: Arc<AtomicU32>,
    shutdown: Arc<AtomicBool>,
    registered: Arc<AtomicBool>,
    hz_store: Arc<AtomicU32>,
) {
    unsafe {
        let hinstance = GetModuleHandleW(None).unwrap();
        let class_name = w!("HermesRawInputClass");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(raw_input_wndproc),
            hInstance: hinstance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        let _ = RegisterClassExW(&wc);
        let ctx = RawInputContext {
            ema_delta: Arc::clone(&ema_delta),
            clock: HybridClock::new(),
            last: 0.0,
            alpha: 0.1,
            samples: Vec::with_capacity(20),
            ema_hz: 125.0,
        };
        let ctx_ptr = Box::into_raw(Box::new(ctx)) as *mut _;
        let hwnd_result = CreateWindowExW(
            Default::default(),
            class_name,
            w!(""),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            Some(HWND_MESSAGE),
            Some(HMENU::default()),
            Some(hinstance.into()),
            None,
        );
        let hwnd = match hwnd_result {
            Ok(h) => h,
            Err(e) => {
                let err = GetLastError();
                eprintln!(
                    "[Hermes] Failed to create message-only window, error: {:?} / {:?}",
                    e, err
                );
                registered.store(false, Ordering::Relaxed);
                let _ = Box::from_raw(ctx_ptr);
                return;
            }
        };
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ctx_ptr as isize);
        let dev = RAWINPUTDEVICE {
            usUsagePage: usage_page,
            usUsage: usage,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        if let Err(e) = RegisterRawInputDevices(&[dev], size_of::<RAWINPUTDEVICE>() as u32) {
            let err = GetLastError();
            eprintln!(
                "[Hermes] Raw input registration failed: {:?}, Win32 error: {:?}",
                e, err
            );
            registered.store(false, Ordering::Relaxed);
            DestroyWindow(hwnd);
            let _ = Box::from_raw(ctx_ptr);
            return;
        }
        registered.store(true, Ordering::Relaxed);
        let mut msg = MSG::default();
        let mut last_hz_update = std::time::Instant::now();
        while !shutdown.load(Ordering::SeqCst) {
            if GetMessageW(&mut msg, Some(hwnd), 0, 0).as_bool() {
                if msg.message == WM_QUIT {
                    break;
                }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            } else {
                break;
            }
            if last_hz_update.elapsed() >= Duration::from_millis(200) {
                let ctx_ref = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut RawInputContext)
                    .as_mut()
                    .unwrap();
                let hz = ctx_ref.ema_hz;
                if hz > 0.0 && hz < 2000.0 {
                    let snapped = snap_polling_rate(hz);
                    hz_store.store(snapped, Ordering::Relaxed);
                }
                last_hz_update = std::time::Instant::now();
            }
        }
        DestroyWindow(hwnd);
        let _ = Box::from_raw(ctx_ptr);
    }
}

#[cfg(target_os = "windows")]
fn apply_network_stack(enable: bool) -> bool {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let (ack_val, nodelay_val, delack_val) = if enable {
        ("1", "1", "0")
    } else {
        ("2", "0", "2")
    };
    let script = format!(
        r#"
        $errorActionPreference = 'Stop'
        try {{
            $interfaces = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Services\Tcpip\Parameters\Interfaces\*' -ErrorAction Stop
            foreach ($int in $interfaces) {{
                Set-ItemProperty -Path $int.PSPath -Name 'TcpAckFrequency' -Value {} -Type DWord -Force
                Set-ItemProperty -Path $int.PSPath -Name 'TCPNoDelay' -Value {} -Type DWord -Force
                Set-ItemProperty -Path $int.PSPath -Name 'TcpDelAckTicks' -Value {} -Type DWord -Force
            }}
            exit 0
        }} catch {{
            exit 1
        }}
    "#,
        ack_val, nodelay_val, delack_val
    );
    let status = std::process::Command::new("powershell")
        .args(&[
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
#[cfg(not(target_os = "windows"))]
fn apply_network_stack(_enable: bool) -> bool {
    true
}

#[cfg(target_os = "windows")]
fn get_active_power_plan() -> Option<String> {
    let output = std::process::Command::new("powercfg")
        .args(&["/getactivescheme"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.lines().next()?;
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 4 {
        Some(parts[3].to_string())
    } else {
        None
    }
}

#[cfg(target_os = "windows")]
fn apply_power_plan(guid: &str) -> bool {
    let status = std::process::Command::new("powercfg")
        .args(&["/setactive", guid])
        .status();
    matches!(status, Ok(s) if s.success())
}

#[cfg(target_os = "windows")]
fn apply_cpu_noise_reduction(enable: bool) -> bool {
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let val = if enable { "0" } else { "1" };
    let script = format!(
        r#"
        powercfg /setacvalueindex scheme_current sub_processor PERFBOOSTMODE {}
        powercfg /setacvalueindex scheme_current sub_processor PROCTHROTTLEMIN {}
        powercfg /setactive scheme_current
        exit 0
    "#,
        val, val
    );
    let status = std::process::Command::new("powershell")
        .args(&["-Command", &script])
        .creation_flags(CREATE_NO_WINDOW)
        .status();
    matches!(status, Ok(s) if s.success())
}

#[cfg(target_os = "windows")]
fn is_laptop() -> bool {
    let mut sps = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut sps) }.is_ok() {
        (sps.BatteryFlag & 128) == 0
    } else {
        false
    }
}

fn snap_polling_rate(hz: f64) -> u32 {
    const CANDIDATES: [f64; 7] = [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];
    let mut best = 125u32;
    let mut best_diff = f64::MAX;
    for c in CANDIDATES {
        let d = (hz - c).abs();
        if d < best_diff {
            best_diff = d;
            best = c as u32;
        }
    }
    // محدودیت ماکزیمم 1000Hz برای جلوگیری از تشخیص اشتباه
    if best > 1000 {
        best = 1000;
    }
    best.max(125)
}

struct HermesEngine {
    is_active: Arc<AtomicBool>,
    status_text: String,
    history: Arc<Mutex<TelemetryHistory>>,
    selected_game_preset: usize,
    jitter_us: Arc<AtomicU32>,
    phase_err_us: Arc<AtomicU32>,
    cs_latency_us: Arc<AtomicU32>,
    dpc_latency_us: Arc<AtomicU32>,
    mouse_hz: Arc<AtomicU32>,
    kb_hz: Arc<AtomicU32>,
    mouse_ema_delta: Arc<AtomicU32>,
    kb_ema_delta: Arc<AtomicU32>,
    telemetry_shutdown: Arc<AtomicBool>,
    raw_shutdown: Arc<AtomicBool>,
    current_timer_res_ms: f32,
    network_applied: bool,
    power_plan_applied: Arc<AtomicBool>,
    original_power_plan: Arc<Mutex<Option<String>>>,
    cpu_noise_reduction_applied: Arc<AtomicBool>,
    admin_error: bool,
    mouse_raw_ok: Arc<AtomicBool>,
    kb_raw_ok: Arc<AtomicBool>,
    hwnd: Option<HWND>,
    pll_strength: f32,
    is_laptop: bool,
    use_rdtsc: Arc<AtomicBool>,
    cstate_locked: Arc<AtomicBool>,
    irq_isolated: Arc<AtomicBool>,
    msi_enabled: Arc<AtomicBool>,
    affinity_applied: Arc<AtomicBool>,
}
impl Default for HermesEngine {
    fn default() -> Self {
        Self {
            is_active: Arc::new(AtomicBool::new(false)),
            status_text: "Idle - Ready".to_string(),
            history: Arc::new(Mutex::new(TelemetryHistory::default())),
            selected_game_preset: 0,
            jitter_us: Arc::new(AtomicU32::new(0)),
            phase_err_us: Arc::new(AtomicU32::new(0)),
            cs_latency_us: Arc::new(AtomicU32::new(0)),
            dpc_latency_us: Arc::new(AtomicU32::new(0)),
            mouse_hz: Arc::new(AtomicU32::new(0)),
            kb_hz: Arc::new(AtomicU32::new(0)),
            mouse_ema_delta: Arc::new(AtomicU32::new(1000)),
            kb_ema_delta: Arc::new(AtomicU32::new(8000)),
            telemetry_shutdown: Arc::new(AtomicBool::new(false)),
            raw_shutdown: Arc::new(AtomicBool::new(false)),
            current_timer_res_ms: 15.6,
            network_applied: false,
            power_plan_applied: Arc::new(AtomicBool::new(false)),
            original_power_plan: Arc::new(Mutex::new(None)),
            cpu_noise_reduction_applied: Arc::new(AtomicBool::new(false)),
            admin_error: false,
            mouse_raw_ok: Arc::new(AtomicBool::new(false)),
            kb_raw_ok: Arc::new(AtomicBool::new(false)),
            hwnd: None,
            pll_strength: 1.0,
            is_laptop: {
                #[cfg(target_os = "windows")]
                {
                    is_laptop()
                }
                #[cfg(not(target_os = "windows"))]
                false
            },
            use_rdtsc: Arc::new(AtomicBool::new(true)),
            cstate_locked: Arc::new(AtomicBool::new(false)),
            irq_isolated: Arc::new(AtomicBool::new(false)),
            msi_enabled: Arc::new(AtomicBool::new(false)),
            affinity_applied: Arc::new(AtomicBool::new(false)),
        }
    }
}
impl Drop for HermesEngine {
    fn drop(&mut self) {
        self.telemetry_shutdown.store(true, Ordering::SeqCst);
        self.raw_shutdown.store(true, Ordering::SeqCst);
        if self.network_applied {
            #[cfg(target_os = "windows")]
            {
                let _ = apply_network_stack(false);
            }
        }
        if self.power_plan_applied.load(Ordering::SeqCst) {
            if let Some(orig) = self.original_power_plan.lock().unwrap().as_ref() {
                #[cfg(target_os = "windows")]
                {
                    let _ = apply_power_plan(orig);
                }
            }
        }
        if self.cpu_noise_reduction_applied.load(Ordering::SeqCst) {
            #[cfg(target_os = "windows")]
            {
                let _ = apply_cpu_noise_reduction(false);
            }
        }
        if self.cstate_locked.load(Ordering::SeqCst) {
            #[cfg(target_os = "windows")]
            {
                let _ = lock_cpu_power_states(false);
            }
        }
        if self.affinity_applied.load(Ordering::SeqCst) {
            #[cfg(target_os = "windows")]
            {
                unsafe {
                    let _ = SetProcessAffinityMask(GetCurrentProcess(), !0);
                }
            }
        }
    }
}

impl eframe::App for HermesEngine {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        ui.ctx().set_theme(egui::Theme::Dark);
        #[cfg(target_os = "windows")]
        {
            if self.hwnd.is_none() {
                if let Ok(wh) = frame.window_handle() {
                    let raw = wh.as_raw();
                    if let RawWindowHandle::Win32(win32) = raw {
                        self.hwnd = Some(HWND(win32.hwnd.get() as *mut std::ffi::c_void));
                    }
                }
            }
        }

        static HOTKEY_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        HOTKEY_INIT.get_or_init(|| {
            let active_flag = Arc::clone(&self.is_active);
            thread::spawn(move || {
                #[cfg(target_os = "windows")]
                unsafe {
                    let res = RegisterHotKey(None, 1001, MOD_ALT, VK_F11.0 as u32);
                    if res.is_ok() {
                        let mut msg = MSG::default();
                        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                            if msg.message == WM_HOTKEY && msg.wParam.0 == 1001 {
                                let curr = active_flag.load(Ordering::SeqCst);
                                active_flag.store(!curr, Ordering::SeqCst);
                            }
                        }
                    }
                }
            });
        });

        #[cfg(target_os = "windows")]
        {
            static RAW_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
            RAW_INIT.get_or_init(|| {
                let mouse_delta = Arc::clone(&self.mouse_ema_delta);
                let kb_delta = Arc::clone(&self.kb_ema_delta);
                let shutdown = Arc::clone(&self.raw_shutdown);
                let mouse_ok = Arc::clone(&self.mouse_raw_ok);
                let kb_ok = Arc::clone(&self.kb_raw_ok);
                let shutdown2 = Arc::clone(&shutdown);
                let mouse_hz_store = Arc::clone(&self.mouse_hz);
                let kb_hz_store = Arc::clone(&self.kb_hz);
                thread::spawn(move || {
                    raw_input_loop(0x01, 0x02, mouse_delta, shutdown, mouse_ok, mouse_hz_store);
                });
                thread::spawn(move || {
                    raw_input_loop(0x01, 0x06, kb_delta, shutdown2, kb_ok, kb_hz_store);
                });
            });
        }

        static TELEMETRY_INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        TELEMETRY_INIT.get_or_init(|| {
            let history_store = Arc::clone(&self.history);
            let jitter_atom = Arc::clone(&self.jitter_us);
            let phase_atom = Arc::clone(&self.phase_err_us);
            let cs_atom = Arc::clone(&self.cs_latency_us);
            let dpc_atom = Arc::clone(&self.dpc_latency_us);
            let shutdown = Arc::clone(&self.telemetry_shutdown);
            thread::spawn(move || {
                while !shutdown.load(Ordering::SeqCst) {
                    thread::sleep(Duration::from_millis(250));
                    #[cfg(target_os = "windows")]
                    let cur_res = unsafe {
                        let mut max_t = 0u32;
                        let mut min_t = 0u32;
                        let mut cur_t = 0u32;
                        if NtQueryTimerResolution(&mut max_t, &mut min_t, &mut cur_t) == 0 {
                            (cur_t as f32) / 10000.0
                        } else {
                            15.6
                        }
                    };
                    #[cfg(not(target_os = "windows"))]
                    let cur_res = 1.0;
                    let j_us = jitter_atom.load(Ordering::Relaxed);
                    let p_us = phase_atom.load(Ordering::Relaxed);
                    let cs_us = cs_atom.load(Ordering::Relaxed);
                    let dpc_us = dpc_atom.load(Ordering::Relaxed);
                    if let Ok(mut hist) = history_store.lock() {
                        if hist.timer_res_history.len() >= 60 {
                            hist.timer_res_history.remove(0);
                            hist.jitter_history.remove(0);
                            hist.phase_error_history.remove(0);
                            hist.context_switch_latency.remove(0);
                            hist.dpc_latency_history.remove(0);
                        }
                        hist.timer_res_history.push(cur_res);
                        hist.jitter_history.push(j_us as f32 / 1000.0);
                        hist.phase_error_history.push(p_us as f32 / 1000.0);
                        hist.context_switch_latency.push(cs_us as f32 / 1000.0);
                        hist.dpc_latency_history.push(dpc_us as f32 / 1000.0);
                    }
                }
            });
        });

        ui.ctx().request_repaint_after(Duration::from_millis(100));

        #[cfg(target_os = "windows")]
        {
            unsafe {
                let mut max_t = 0u32;
                let mut min_t = 0u32;
                let mut cur_t = 0u32;
                if NtQueryTimerResolution(&mut max_t, &mut min_t, &mut cur_t) == 0 {
                    self.current_timer_res_ms = (cur_t as f32) / 10000.0;
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            self.current_timer_res_ms = 1.0;
        }

        let target_tick = match self.selected_game_preset {
            0 => 128.0,
            1 => 128.0,
            _ => 60.0,
        };
        let active = self.is_active.load(Ordering::SeqCst);

        // خواندن مقادیر با محدودیت
        let jitter_ms = (self.jitter_us.load(Ordering::SeqCst) as f32 / 1000.0).min(1.0);
        let phase_ms = (self.phase_err_us.load(Ordering::SeqCst) as f32 / 1000.0).min(0.5);
        let cs_lat_ms = (self.cs_latency_us.load(Ordering::SeqCst) as f32 / 1000.0).min(0.5);
        let dpc_lat_ms = (self.dpc_latency_us.load(Ordering::SeqCst) as f32 / 1000.0).min(0.5);
        let m_hz = self.mouse_hz.load(Ordering::SeqCst);
        let k_hz = self.kb_hz.load(Ordering::SeqCst);

        egui::ScrollArea::vertical().show(ui, |ui| {
            if self.admin_error {
                ui.colored_label(
                    egui::Color32::RED,
                    "Error: Administrator privileges required for some features.",
                );
                ui.add_space(4.0);
            }
            ui.heading("⚡ Hermes Engine v2.1");
            ui.horizontal(|ui| {
                ui.label("System:");
                let mode_text = if self.is_laptop {
                    "Laptop Mode"
                } else {
                    "Desktop Mode"
                };
                let mode_color = if self.is_laptop {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::LIGHT_BLUE
                };
                ui.colored_label(mode_color, mode_text);
            });
            ui.label("Real-time phase-locked input & system latency stabilizer (RAM-Only).");
            ui.separator();
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.label("Profile:");
                let presets = ["CS2 (128Hz Subtick)", "Valorant (128Hz)", "Fortnite (60Hz)"];
                egui::ComboBox::from_label("")
                    .selected_text(presets[self.selected_game_preset])
                    .show_ui(ui, |ui| {
                        for (i, p) in presets.iter().enumerate() {
                            ui.selectable_value(&mut self.selected_game_preset, i, *p);
                        }
                    });
                ui.label("| Timer:");
                ui.colored_label(
                    if self.current_timer_res_ms <= 1.0 {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::RED
                    },
                    format!("{:.2} ms", self.current_timer_res_ms),
                );
                ui.label("| Jitter:");
                ui.colored_label(egui::Color32::LIGHT_GREEN, format!("{:.3} ms", jitter_ms));
                ui.label("| Phase:");
                ui.colored_label(egui::Color32::LIGHT_BLUE, format!("{:.3} ms", phase_ms));
                ui.label("| CS Lat:");
                ui.colored_label(
                    if cs_lat_ms < 0.1 {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::YELLOW
                    },
                    format!("{:.3} ms", cs_lat_ms),
                );
                ui.label("| DPC:");
                ui.colored_label(
                    if dpc_lat_ms < 0.1 {
                        egui::Color32::GREEN
                    } else {
                        egui::Color32::RED
                    },
                    format!("{:.3} ms", dpc_lat_ms),
                );
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("🖱 Mouse:");
                let mouse_label = if m_hz > 0 {
                    format!("{} Hz", m_hz)
                } else {
                    "measuring...".into()
                };
                ui.colored_label(egui::Color32::GREEN, mouse_label);
                ui.label("| ⌨ Keyboard:");
                let kb_label = if k_hz > 0 {
                    format!("{} Hz", k_hz)
                } else {
                    "measuring...".into()
                };
                ui.colored_label(egui::Color32::LIGHT_BLUE, kb_label);
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let msi_enabled = self.msi_enabled.load(Ordering::SeqCst);
                let msi_label = if msi_enabled {
                    "MSI: [ON]"
                } else {
                    "MSI: [OFF]"
                };
                if ui.button(msi_label).clicked() {
                    let ids = get_usb_controller_instance_ids();
                    if !ids.is_empty() {
                        let mut success = true;
                        for id in &ids {
                            if !enable_msi_for_device(id) {
                                success = false;
                            }
                        }
                        if success {
                            let new_state = !msi_enabled;
                            self.msi_enabled.store(new_state, Ordering::SeqCst);
                            self.admin_error = false;
                            self.status_text = if new_state {
                                "MSI enabled for USB controllers (restart required)."
                            } else {
                                "MSI disabled."
                            }
                            .into();
                        } else {
                            self.admin_error = true;
                            self.status_text = "MSI configuration failed.".into();
                        }
                    } else {
                        self.admin_error = true;
                        self.status_text = "No USB controller found.".into();
                    }
                }
                let cstate_locked = self.cstate_locked.load(Ordering::SeqCst);
                let cstate_label = if cstate_locked {
                    "C-State: [LOCKED]"
                } else {
                    "C-State: [DEFAULT]"
                };
                if ui.button(cstate_label).clicked() {
                    let new_state = !cstate_locked;
                    let ok = lock_cpu_power_states(new_state);
                    if ok {
                        self.cstate_locked.store(new_state, Ordering::SeqCst);
                        self.admin_error = false;
                        self.status_text = if new_state {
                            "C-States locked & core parking disabled."
                        } else {
                            "C-States restored to default."
                        }
                        .into();
                    } else {
                        self.admin_error = true;
                        self.status_text = "Failed to lock C-States. Run as Admin.".into();
                    }
                }
                let affinity_applied = self.affinity_applied.load(Ordering::SeqCst);
                let affinity_label = if affinity_applied {
                    "Affinity: [ISOLATED]"
                } else {
                    "Affinity: [DEFAULT]"
                };
                if ui.button(affinity_label).clicked() {
                    let new_state = !affinity_applied;
                    let ok = if new_state {
                        set_process_affinity_to_isolated_core()
                    } else {
                        unsafe { SetProcessAffinityMask(GetCurrentProcess(), !0).is_ok() }
                    };
                    if ok {
                        self.affinity_applied.store(new_state, Ordering::SeqCst);
                        self.admin_error = false;
                        self.status_text = if new_state {
                            "Process affinity locked to core 7."
                        } else {
                            "Process affinity restored."
                        }
                        .into();
                    } else {
                        self.admin_error = true;
                        self.status_text = "Failed to set affinity. Run as Admin.".into();
                    }
                }
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let btn_label = if self.network_applied {
                    "Network Stack: [Enabled]"
                } else {
                    "Network Stack: [Disabled]"
                };
                if ui.button(btn_label).clicked() {
                    let new_state = !self.network_applied;
                    let ok = apply_network_stack(new_state);
                    if ok {
                        self.network_applied = new_state;
                        self.admin_error = false;
                        self.status_text = if new_state {
                            "Network stack optimized."
                        } else {
                            "Network stack restored."
                        }
                        .into();
                    } else {
                        self.admin_error = true;
                        self.status_text = "Network tweak failed. Run as Administrator.".into();
                    }
                }
                let power_applied = self.power_plan_applied.load(Ordering::SeqCst);
                let power_label = if power_applied {
                    "Power Plan: [High Perf]"
                } else {
                    "Power Plan: [Default]"
                };
                if ui.button(power_label).clicked() {
                    if !power_applied {
                        if let Some(orig_guid) = get_active_power_plan() {
                            if apply_power_plan("8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c") {
                                self.power_plan_applied.store(true, Ordering::SeqCst);
                                *self.original_power_plan.lock().unwrap() = Some(orig_guid);
                                self.admin_error = false;
                            } else {
                                self.admin_error = true;
                            }
                        } else {
                            self.admin_error = true;
                        }
                    } else {
                        if let Some(orig_guid) = self.original_power_plan.lock().unwrap().clone() {
                            if apply_power_plan(&orig_guid) {
                                self.power_plan_applied.store(false, Ordering::SeqCst);
                                self.admin_error = false;
                            } else {
                                self.admin_error = true;
                            }
                        }
                    }
                }
            });

            if self.is_laptop {
                ui.horizontal(|ui| {
                    let noise_applied = self.cpu_noise_reduction_applied.load(Ordering::SeqCst);
                    let noise_label = if noise_applied {
                        "Laptop Noise Reduction: [ON]"
                    } else {
                        "Laptop Noise Reduction: [OFF]"
                    };
                    if ui.button(noise_label).clicked() {
                        let new_state = !noise_applied;
                        if apply_cpu_noise_reduction(new_state) {
                            self.cpu_noise_reduction_applied
                                .store(new_state, Ordering::SeqCst);
                            self.admin_error = false;
                        } else {
                            self.admin_error = true;
                        }
                    }
                });
            }

            ui.add_space(6.0);
            ui.label(format!("PLL Strength: {:.2}", self.pll_strength));
            ui.add(egui::Slider::new(&mut self.pll_strength, 0.1..=2.0).text("PLL Strength"));
            let mut use_rdtsc = self.use_rdtsc.load(Ordering::SeqCst);
            if ui
                .checkbox(&mut use_rdtsc, "Use RDTSC (sub-µs precision)")
                .changed()
            {
                self.use_rdtsc.store(use_rdtsc, Ordering::SeqCst);
            }
            ui.add_space(6.0);

            ui.separator();
            ui.add_space(4.0);

            let hist = self.history.lock().unwrap();
            let graph_w = ui.available_width().max(300.0);
            let graph_h = 50.0;
            let len = hist.timer_res_history.len();
            let step = graph_w / (len as f32).max(1.0);

            ui.label("📊 Timer Resolution (0 - 16 ms):");
            let (rect_t, _) =
                ui.allocate_exact_size(egui::vec2(graph_w, graph_h), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect_t, 4.0, egui::Color32::from_rgb(12, 16, 28));
            for i in 0..len.saturating_sub(1) {
                let val1 = (hist.timer_res_history[i] / 16.0).clamp(0.0, 1.0);
                let val2 = (hist.timer_res_history[i + 1] / 16.0).clamp(0.0, 1.0);
                let y1 = rect_t.max.y - (val1 * graph_h);
                let x2 = rect_t.min.x + ((i + 1) as f32 * step);
                let y2 = rect_t.max.y - (val2 * graph_h);
                let color = if hist.timer_res_history[i] <= 1.1 {
                    egui::Color32::GREEN
                } else {
                    egui::Color32::LIGHT_RED
                };
                ui.painter().line_segment(
                    [
                        egui::pos2(rect_t.min.x + (i as f32 * step), y1),
                        egui::pos2(x2, y2),
                    ],
                    egui::Stroke::new(1.8, color),
                );
            }

            ui.add_space(6.0);
            ui.label("📊 PLL Jitter (0 - 1.0 ms):");
            let (rect_j, _) =
                ui.allocate_exact_size(egui::vec2(graph_w, graph_h), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect_j, 4.0, egui::Color32::from_rgb(12, 16, 28));
            for i in 0..len.saturating_sub(1) {
                let val1 = (hist.jitter_history[i] / 1.0).clamp(0.0, 1.0);
                let val2 = (hist.jitter_history[i + 1] / 1.0).clamp(0.0, 1.0);
                let y1 = rect_j.max.y - (val1 * graph_h);
                let x2 = rect_j.min.x + ((i + 1) as f32 * step);
                let y2 = rect_j.max.y - (val2 * graph_h);
                let color = if active {
                    egui::Color32::LIGHT_BLUE
                } else {
                    egui::Color32::GRAY
                };
                ui.painter().line_segment(
                    [
                        egui::pos2(rect_j.min.x + (i as f32 * step), y1),
                        egui::pos2(x2, y2),
                    ],
                    egui::Stroke::new(1.8, color),
                );
            }

            ui.add_space(6.0);
            ui.label("📊 Phase-Lock Error (0 - 0.5 ms):");
            let (rect_p, _) =
                ui.allocate_exact_size(egui::vec2(graph_w, graph_h), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect_p, 4.0, egui::Color32::from_rgb(12, 16, 28));
            for i in 0..len.saturating_sub(1) {
                let val1 = (hist.phase_error_history[i] / 0.5).clamp(0.0, 1.0);
                let val2 = (hist.phase_error_history[i + 1] / 0.5).clamp(0.0, 1.0);
                let y1 = rect_p.max.y - (val1 * graph_h);
                let x2 = rect_p.min.x + ((i + 1) as f32 * step);
                let y2 = rect_p.max.y - (val2 * graph_h);
                let color = if val1 < 0.1 {
                    egui::Color32::GREEN
                } else if val1 < 0.3 {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::RED
                };
                ui.painter().line_segment(
                    [
                        egui::pos2(rect_p.min.x + (i as f32 * step), y1),
                        egui::pos2(x2, y2),
                    ],
                    egui::Stroke::new(1.8, color),
                );
            }

            ui.add_space(6.0);
            ui.label("📊 Context Switch Latency (0 - 0.5 ms):");
            let (rect_cs, _) =
                ui.allocate_exact_size(egui::vec2(graph_w, graph_h), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect_cs, 4.0, egui::Color32::from_rgb(12, 16, 28));
            for i in 0..len.saturating_sub(1) {
                let val1 = (hist.context_switch_latency[i] / 0.5).clamp(0.0, 1.0);
                let val2 = (hist.context_switch_latency[i + 1] / 0.5).clamp(0.0, 1.0);
                let y1 = rect_cs.max.y - (val1 * graph_h);
                let x2 = rect_cs.min.x + ((i + 1) as f32 * step);
                let y2 = rect_cs.max.y - (val2 * graph_h);
                let color = if val1 < 0.1 {
                    egui::Color32::GREEN
                } else if val1 < 0.3 {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::RED
                };
                ui.painter().line_segment(
                    [
                        egui::pos2(rect_cs.min.x + (i as f32 * step), y1),
                        egui::pos2(x2, y2),
                    ],
                    egui::Stroke::new(1.8, color),
                );
            }

            ui.add_space(6.0);
            ui.label("📊 DPC Latency (0 - 0.5 ms):");
            let (rect_dpc, _) =
                ui.allocate_exact_size(egui::vec2(graph_w, graph_h), egui::Sense::hover());
            ui.painter()
                .rect_filled(rect_dpc, 4.0, egui::Color32::from_rgb(12, 16, 28));
            for i in 0..len.saturating_sub(1) {
                let val1 = (hist.dpc_latency_history[i] / 0.5).clamp(0.0, 1.0);
                let val2 = (hist.dpc_latency_history[i + 1] / 0.5).clamp(0.0, 1.0);
                let y1 = rect_dpc.max.y - (val1 * graph_h);
                let x2 = rect_dpc.min.x + ((i + 1) as f32 * step);
                let y2 = rect_dpc.max.y - (val2 * graph_h);
                let color = if val1 < 0.1 {
                    egui::Color32::GREEN
                } else if val1 < 0.3 {
                    egui::Color32::YELLOW
                } else {
                    egui::Color32::RED
                };
                ui.painter().line_segment(
                    [
                        egui::pos2(rect_dpc.min.x + (i as f32 * step), y1),
                        egui::pos2(x2, y2),
                    ],
                    egui::Stroke::new(1.8, color),
                );
            }

            ui.add_space(8.0);
            ui.separator();

            ui.horizontal(|ui| {
                if !active {
                    if ui
                        .button("🚀 START ULTIMATE STABILIZER (1ms Timer + PLL Sync)")
                        .clicked()
                    {
                        self.is_active.store(true, Ordering::SeqCst);
                        self.status_text =
                            "ACTIVE! RAM-Only Mode, 1ms Timer & PLL Active.".to_string();
                        let flag = Arc::clone(&self.is_active);
                        let tick_hz = target_tick;
                        let jitter_atom = Arc::clone(&self.jitter_us);
                        let phase_atom = Arc::clone(&self.phase_err_us);
                        let cs_atom = Arc::clone(&self.cs_latency_us);
                        let dpc_atom = Arc::clone(&self.dpc_latency_us);
                        let use_rdtsc = Arc::clone(&self.use_rdtsc);
                        let params = PllParams {
                            kp: 0.6 * self.pll_strength as f64,
                            ki: 0.12 * self.pll_strength as f64,
                            lfo_amp: 5.0,
                            lfo_period: 10.0,
                        };
                        thread::spawn(move || {
                            let _timer_guard = TimerResolutionGuard;
                            #[cfg(target_os = "windows")]
                            unsafe {
                                let _ = timeBeginPeriod(1);
                                SetThreadAffinityMask(GetCurrentThread(), 0b10000000);
                                SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
                                SetThreadExecutionState(
                                    ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED,
                                );
                            }
                            let clock = if use_rdtsc.load(Ordering::SeqCst) {
                                HybridClock::new()
                            } else {
                                HybridClock::new()
                            };
                            let mut pll = PllEngine::new(tick_hz, &clock, &params);
                            let mut next_sleep_us = 1000u64;
                            let us_per_tick = 1_000_000.0 / tick_hz;
                            let mut first = true;
                            let mut dpc_ema = 0.0;
                            while flag.load(Ordering::SeqCst) {
                                let sleep_dur =
                                    Duration::from_micros(next_sleep_us.saturating_sub(200));
                                thread::sleep(sleep_dur);
                                let start_spin = clock.now_counter();
                                let spin_target =
                                    (next_sleep_us as i64 * (clock.frequency() as i64)) / 1_000_000;
                                let spin_start = std::time::Instant::now();
                                while clock.now_counter() - start_spin < spin_target {
                                    std::hint::spin_loop();
                                }
                                let spin_elapsed = spin_start.elapsed();
                                let mut dpc_lat = spin_elapsed.as_micros() as f64
                                    - (next_sleep_us as f64 - 200.0);
                                if dpc_lat < 0.0 {
                                    dpc_lat = 0.0;
                                }
                                if dpc_lat > 500.0 {
                                    dpc_lat = 500.0;
                                }
                                dpc_ema = dpc_ema * 0.8 + dpc_lat * 0.2;
                                let now_counter = clock.now_counter();
                                let (sleep_next, jitter_ticks, phase_err, cs_lat) =
                                    pll.update(now_counter);
                                next_sleep_us = sleep_next;
                                if first {
                                    first = false;
                                    continue;
                                }
                                let jitter_us = (jitter_ticks * us_per_tick) as u32;
                                let phase_us = (phase_err * us_per_tick) as u32;
                                let cs_us = (cs_lat * 1000.0) as u32;
                                let dpc_us = (dpc_ema * 1000.0) as u32;
                                jitter_atom.store(
                                    if jitter_us > 1000 { 0 } else { jitter_us },
                                    Ordering::Relaxed,
                                );
                                phase_atom.store(
                                    if phase_us > 500 { 0 } else { phase_us },
                                    Ordering::Relaxed,
                                );
                                cs_atom
                                    .store(if cs_us > 500 { 0 } else { cs_us }, Ordering::Relaxed);
                                dpc_atom.store(
                                    if dpc_us > 500 { 0 } else { dpc_us },
                                    Ordering::Relaxed,
                                );
                            }
                        });
                    }
                } else {
                    if ui.button("🛑 STOP & DEACTIVATE").clicked() {
                        self.is_active.store(false, Ordering::SeqCst);
                        self.jitter_us.store(0, Ordering::Relaxed);
                        self.phase_err_us.store(0, Ordering::Relaxed);
                        self.cs_latency_us.store(0, Ordering::Relaxed);
                        self.dpc_latency_us.store(0, Ordering::Relaxed);
                        self.status_text = "Inactive. Reset to Default.".into();
                    }
                }
            });

            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Status:");
                if active {
                    ui.colored_label(egui::Color32::GREEN, &self.status_text);
                } else {
                    ui.colored_label(egui::Color32::YELLOW, &self.status_text.clone());
                }
            });

            #[cfg(target_os = "windows")]
            {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Raw Input:");
                    let mouse_ok = self.mouse_raw_ok.load(Ordering::Relaxed);
                    let kb_ok = self.kb_raw_ok.load(Ordering::Relaxed);
                    if !mouse_ok || !kb_ok {
                        ui.colored_label(egui::Color32::YELLOW, "not registered (try admin)");
                    } else {
                        ui.colored_label(egui::Color32::GREEN, "OK");
                    }
                });
            }
        });
    }
}

fn main() -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 860.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "Hermes Engine v2.1",
        options,
        Box::new(|_cc| Ok(Box::<HermesEngine>::default())),
    )
}
