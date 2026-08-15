//! Hermes Engine Ultimate — measurement, adaptive waiting, and the
//! 3-layer PLL phase engine.
//!
//! The crate measures scheduler wake-up error for its own worker and
//! optionally drives a reversible, admin-free set of Windows timing
//! optimizations. It does not claim to measure electrical ripple,
//! DPC/ISR duration, game input latency, or network latency directly.

pub mod benchmark;
pub mod config;
pub mod games;
pub mod hotkey;
pub mod input;
pub mod network;
pub mod platform;
pub mod pll;
pub mod system_mod;
pub mod telemetry;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
