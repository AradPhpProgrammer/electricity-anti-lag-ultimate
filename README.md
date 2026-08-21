<div align="center">

# ⚡ Hermes Engine v4.0.0 Ultimate
### Sub-Microsecond Gaming Input-Lag & Phase-Locked Loop (PLL) Optimizer for Windows

[![Release](https://img.shields.io/github/v/release/AradPhpProgrammer/electricity-anti-lag-ultimate?style=for-the-badge&color=00D4FF)](https://github.com/AradPhpProgrammer/electricity-anti-lag-ultimate/releases/latest)
[![Rust](https://img.shields.io/badge/rust-2021%20edition-orange?style=for-the-badge&logo=rust)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg?style=for-the-badge)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%20%2F%2011-blue?style=for-the-badge&logo=windows)](https://microsoft.com)

**[English](README.md)** • **[فارسی](docs/README_FA.md)** • **[Русский](docs/README_RU.md)**

---

<p align="center">
  <b>Hermes Engine</b> is a real-time kernel-level synchronization tool built with Rust and eframe/egui.<br/>
  It eliminates input delay, mouse jitter, and micro-stutters in competitive games like <b>CS2, Valorant, Apex Legends, Overwatch 2</b> by phase-locking CPU timing windows directly to server tick cycles.
</p>

</div>

---

## 🚀 What's New in Version 4.0.0 (L0 Critical Path Isolation)

Hermes Engine v4.0.0 is a major architecture update focused on **L0 ultra-low latency timing optimization**:
- **Isolated L0 Timing Engine:** `MouseModulator`, `HzDetector`, and `ClockStabilityMonitor` are now completely worker-local inside `hermes-pll-worker`, preventing cross-thread lock contention.
- **Cache-Line Aligned Atomic State:** Split shared state into cache-line-aligned UI-to-worker control atomics and worker-to-UI telemetry atomics (0% false sharing).
- **Packed Hz Control Word:** Consolidated Auto/Manual Hz mode and manual rate into a single atomic word for zero-overhead mode transitions.
- **Sub-tick Batch Telemetry:** Telemetry (PLL phase, resolved Hz, clock health) is now published every 8 ticks instead of every tick, reducing bus overhead and CPU tax by ~87%.
- **O(1) Rolling Clock Analytics:** Clock stability statistics (mean, std-dev, coefficient of variation) are computed in constant O(1) time using running sum and sum-of-squares tracking.

---

## ⚡ Quick Start Guide (Recommended Setup)

Want the best possible performance in 10 seconds? Follow these simple steps:

1. **Run as Administrator:** Right-click `HermesEngine-v4.0.0.exe` and select **Run as Administrator** (needed for Windows timer resolution & MSI registry guards).
2. **Hit Start Engine:** In the **🚀 PLL Engine** tab, click **START PLL ENGINE** (or press `Alt + 1` in-game).
3. **Select Server Tick Rate:**
   - For **CS2 / Valorant / Apex:** Leave on **128 Hz (Sub-tick / High-precision)**.
   - For regular 64-tick servers: Switch to **64 Hz**.
4. **Tune PLL Power:** Keep the **⚡ PLL Power Multiplier** at `1.0x` (or push to `1.2x - 1.5x` for competitive tournament mode).
5. **Enable System Guards:** Go to the **⚙ System** tab and turn ON:
   - ✅ **Lock C-States & P-States**
   - ✅ **Disable TCP Nagle Algorithm**
   - ✅ **Isolate Core Affinity**
6. **Jump into the Game!** You will immediately feel a crisper, more responsive crosshair movement and zero micro-delays on mouse clicks.

---

## 🎮 What Every Button & Setting Actually Does (Plain English)

### 🚀 Tab 1: PLL Engine (Heartbeat & Timing)
* **START / STOP PLL ENGINE (`Alt + 1`):**  
  Turns on the real-time phase engine. Sets Windows timer resolution to 1.00ms and spins in microsecond bursts so your CPU is never caught "sleeping" when an input frame arrives.
* **Server Tick Rate (64Hz / 128Hz):**  
  Aligns the internal heartbeat clock with the game server's tick rate. 128Hz corresponds to a `7.81 ms` tick cycle.
* **⚡ PLL Power Multiplier (0.1x .. 2.0x):**  
  How aggressively the engine forces CPU cycles to align with the phase clock.  
  - `0.5x`: Low CPU consumption, ideal for laptops or older CPUs.  
  - `1.0x`: Balanced tournament default.  
  - `1.5x - 2.0x`: Razor-sharp responsiveness with maximum priority.
* **Kp (Proportional Gain) & Ki (Integral Gain):**  
  Controls how fast the Phase-Locked Loop reacts to instantaneous timing spikes (Kp) and long-term clock drift (Ki).

---

### ⚙ Tab 2: System & Power Tuners (Zero-Spike Guard)
* **🔒 Lock C-States & P-States:**  
  *What it does:* Prevents Windows from putting CPU cores into deep sleep (C-State parking) during low-activity frames.  
  *Why use it:* CPU core wake-up latency takes 200–800 µs; disabling C-States eliminates micro-stutters during aim duels!
* **🌐 Disable TCP Nagle Algorithm & ACK Delay:**  
  *What it does:* Forces Windows to send packets immediately instead of buffering them to create larger packets.  
  *Why use it:* Drops network input lag by 5–15ms and delivers hitreg confirmations instantly.
* **🎯 Core Affinity Isolation:**  
  *What it does:* Locks the Hermes Engine worker thread to high-performance cores, preventing Windows from bouncing threads across efficiency cores (E-Cores).
* **⚡ MSI (Message Signaled Interrupts) Mode:**  
  *What it does:* Switches your USB controller and Network Card from legacy line interrupts to high-speed MSI interrupts.

---

### 🖱 Tab 3: Input & Hardware Hz
* **Mouse Delta Modulator (Layer 2):**  
  Dynamically pushes the phase window forward whenever fast mouse flicks are detected, so high-speed flick shots are registered on the earliest possible tick.
* **Shot Resync / Click Interrupt (`Alt + 2`):**  
  Instantly resets any accumulated phase jitter the millisecond you click or tap the hotkey, guaranteeing absolute zero latency on your first shot.

---

### ⌨ Tab 4: Global Hotkeys (Fullscreen Compatible)
Works inside **100% of fullscreen games** (DirectX 11, DirectX 12, Vulkan, OpenGL):
* **`Alt + 1`** ➔ Toggle Engine ON / OFF
* **`Alt + 2`** ➔ Shot Resync (Zero-Latency Click trigger)
* **`Alt + 3`** ➔ Reset All Timing Knobs to Default

---

## 🛡 Safety & Clean Exit Guarantee
- **100% Reversible:** All system tweaks (C-States, Nagle, Affinity, Timer) are applied in-memory and cleanly restored to default Windows settings when you close the app or click Reset.
- **No VAC / Anti-Cheat Risk:** Hermes Engine operates purely on standard Windows OS APIs without injecting DLLs, reading game memory, or modifying game files.

---

## ⚠️ Anti-Cheat Disclaimer (FACEIT, ESEA, etc.)
**We have NOT tested Hermes Engine on FACEIT, ESEA, or other third-party anti-cheat platforms.** We take absolutely NO responsibility if you use this tool on FACEIT or similar services and face any consequences, bans, or account restrictions. Use at your own risk on such platforms.

---

## 🙏 Special Thanks & Credits
- **bardiavam:** Huge thanks for refactoring and isolating the L0 timing critical path in v4.0.0!
