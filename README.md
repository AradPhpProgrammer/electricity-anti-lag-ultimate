# ⚡ Electricity Anti-Lag Ultimate Master (v1.0.0)

> **Advanced Input Voltage Stabilizer, Electrical Noise Suppression & PLL Phase Sync Engine for Competitive Gaming (CS2, Valorant, Apex, Fortnite) written in Rust.**

[![Rust](https://img.shields.io/badge/Language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Windows-blue.svg)](https://www.microsoft.com/windows)
[![License](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

---

### 🌐 Select Language / Выберите язык / انتخاب زبان
| Language | Flag / Emblem | Link |
| :--- | :---: | :--- |
| **English** | 🇬🇧 | *Current Page* |
| **Persian (فارسی)** | 🦁☀️ | [مشاهده راهنمای فارسی](docs/README_FA.md) |
| **Russian (Русский)** | 🇷🇺 | [Читать на русском](docs/README_RU.md) |

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

## 🛠 Building from Source
Ensure you have the Rust toolchain installed:


The compiled executable will be available at .

---

## 📄 License
Distributed under the **MIT License**. See  for more information.
