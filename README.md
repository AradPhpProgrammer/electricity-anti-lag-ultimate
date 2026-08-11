# ⚡ Electricity Anti-Lag Ultimate Master (v1.0.0)

> **Advanced Input Voltage Stabilizer, Electrical Noise Suppression & PLL Phase Sync Engine for Competitive Gaming (CS2, Valorant, Apex, Fortnite) written in Rust.**

[![Rust](https://img.shields.io/badge/Language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Windows-blue.svg)](https://www.microsoft.com/windows)
[![Release](https://img.shields.io/badge/Release-v1.0.0-blueviolet.svg)](https://github.com/AradPhpProgrammer/electricity-anti-lag-ultimate/releases/tag/v1.0.0)
[![License](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)


## ⚠️ IMPORTANT NOTICE / DISCLAIMER
> This tool modifies low-level system timers, process thread priorities, and CPU affinity. **DO NOT use this tool on servers protected by kernel-level anti-cheats such as FACEIT, ESEA, or Vanguard.** Doing so may result in an immediate hardware ID (HWID) ban. The authors and maintainers assume **no responsibility** for any bans or system instability incurred by using this software on restricted platforms.

---

### 🌐 Select Language / Выберите язык / انتخاب زبان
| Language | Flag / Emblem | Link |
| :--- | :---: | :--- |
| **English** | 🇬🇧 | *Current Page* |
| **Persian (فارسی)** | 🦁☀️ | [مشاهده راهنمای فارسی](docs/README_FA.md) |
| **Russian (Русский)** | 🇷🇺 | [Читать на русском](docs/README_RU.md) |

---

## 🚀 Quick Download
Download the ready-to-use optimized executable directly from GitHub Releases:
- **[Download electricity_anti_lag_ultimate.exe (v1.0.0)](https://github.com/AradPhpProgrammer/electricity-anti-lag-ultimate/releases/download/v1.0.0/electricity_anti_lag_ultimate.exe)**

---

## 📖 Overview
In competitive FPS gaming (e.g. **Counter-Strike 2**, **Valorant**), micro-variations in USB controller power delivery, laptop power adapter ripple noise, and Windows timer quantization induce subtle input phase jitter and spatial input lag.

**Electricity Anti-Lag Ultimate** mitigates electrical input lag and unifies key/mouse event dispatching via low-level Win32 system APIs and Phase-Locked Loop (PLL) synchronization.

---

## ✨ Key Features
- **1ms Windows Hardware Timer Resolution:** Enforces  to eliminate scheduler frame jitter.
- **PLL Phase Sync Engine:** Synchronizes mouse/keyboard event frames with competitive tickrate profiles (CS2 128Hz Subtick, Valorant 128Hz, Fortnite 60Hz).
- **Auto Polling Rate Detection & Monotonic Latching:** Dynamically detects up to 8000Hz polling rates without dropping during idle pauses.
- **Thread Core Affinity & Real-time Isolation:** Executes on dedicated background core affinity () with zero frame drops.
- **Global In-Game Hotkey ():** Seamlessly toggle state inside active fullscreen CS2 matches via native .
- **100% RAM-Only Architecture:** Zero disk writes, zero registry entries, instant reset on application exit.

---

## 📊 How to Benchmark & Prove Input Lag Reduction
To scientifically prove the effectiveness of **Electricity Anti-Lag Ultimate** in competitive games like CS2, use the following methods:

### 1. Windows Timer Resolution Check (TimerTool / PowerShell)
By default, Windows operates on a 15.6ms timer quantum. When active, this tool forces 1ms.
- Run  or check with PowerShell to confirm system timer resolution drops from  to .

### 2. CS2 Built-in Latency Graph ( / )
- In CS2, enable  or the performance overlay.
- Compare frame time variance () and input processing latency with the stabilizer ON vs. OFF during rapid mouse flicking. You will notice a reduction in frame time spikes (Micro-stutter removal).

### 3. LatencyMon (DPC & ISR Latency)
- Run  while gaming with and without the tool. Notice the reduction in highest DPC routine execution time and ISR latency spikes.

---

## 🛠 Building from Source
Ensure you have the Rust toolchain installed:


The compiled executable will be available at .

---

## 📄 License
Distributed under the **MIT License**. See  for more information.

---
**Tags:** #rust #cs2 #anti-lag #gaming #latency-optimization #windows-api #polling-rate #pll-sync #input-lag
