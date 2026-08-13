# ⚡ Electricity Anti-Lag Ultimate (Hermes Phase Engine) - v2.0.0

[![Version](https://img.shields.io/badge/version-2.0.0-blue.svg)](https://github.com/AradPhpProgrammer/electricity-anti-lag-ultimate/releases/tag/v2.0.0)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Language](https://img.shields.io/badge/Language-Rust-orange.svg)](https://www.rust-lang.org/)

English | [فارسی](docs/README_FA.md) | [Русский](docs/README_RU.md)

---

> ⚠️ **FACEIT / Anti-Cheat Disclaimer**: This tool changes low-level Windows timer resolution, process affinities, and C-state power management. While safe and non-intrusive for standard usage, always check your specific game's anti-cheat terms of service (such as FACEIT, Vanguard, or EAC) before running during official competitive matches.

---

## 🚀 What's New in Version 2.0.0 (Masterpiece Update)

Version 2.0.0 is a complete overhaul of the Phase Lock Loop (PLL) synchronization core, delivering unprecedented stability and system-wide input latency reduction for high-tick games (CS2, Valorant, Apex Legends).

### 🔍 Key Architectural Enhancements in v2.0.0:
1. **Hybrid TSC/QPC Clock & Hardware Auto-Hz Sync**: Real-time Hardware polling (up to 1000Hz) dynamically synced with raw input mouse interrupts.
2. **Brownian LFO Jitter Suppression**: Active suppression of dirty mains power fluctuation noise and electrical DPC interference.
3. **C-State & Power Plan Lock**: Automatic lockdown of CPU power states to eliminate micro-stutters caused by frequency scaling.
4. **MSI (Message Signaled Interrupts) Enforcer**: One-click driver-level MSI activation for all USB controllers to bypass legacy interrupt lines.
5. **Real Absolute-Scale Telemetry**: Honest microsecond-accurate graphs for Jitter, DPC Latency, CS Latency, and Phase Error.

---

## ⚡ Direct Download

Download the pre-compiled, stand-alone Windows executable directly from our Releases:
👉 **[Download Hermes Engine v2.0.0 Executable (`hermes-engine.exe`)](https://github.com/AradPhpProgrammer/electricity-anti-lag-ultimate/releases/download/v2.0.0/hermes-engine.exe)**

---

## 📊 Benchmarking & Verification Guide

To verify the input latency reduction and frame-time consistency:
1. Launch your target game (e.g., Counter-Strike 2).
2. Run `hermes-engine.exe` as Administrator.
3. Observe the real-time telemetry:
   - **Phase Error**: Below `0.05ms`
   - **Context Switch Latency**: Near `0.00ms`
   - **Timer Resolution**: Locked at `0.5ms` (0.499ms)

---

## 🛠️ Building from Source

### Prerequisites
- Windows 10 / 11 64-bit
- Rust Toolchain (`x86_64-pc-windows-msvc` or `x86_64-pc-windows-gnu`)

```bash
git clone https://github.com/AradPhpProgrammer/electricity-anti-lag-ultimate.git
cd electricity-anti-lag-ultimate
cargo build --release
```

The compiled binary will be located at `target/release/electricity_anti_lag_ultimate.exe`.

---

## 🔮 What's Coming Next in v3.0.0

Future releases will focus on **extreme kernel-level optimizations**:
- **Direct Kernel-level Interrupt Routing**: Bypassing Windows user-mode messaging queue for mouse inputs.
- **Zero-Alloc Threading Pipeline**: Reducing Memory Controller latency under heavy CPU loads.
- **Custom Hardware Profile Presets**: One-click tuning for Intel Core Ultra / AMD Ryzen 3D V-Cache processors.

---

## 📄 License

This project is licensed under the [MIT License](LICENSE).
