use hermes_engine::benchmark::{run_comparison, BenchmarkResult};
use hermes_engine::telemetry::Preset;
use std::path::PathBuf;

#[derive(Debug)]
struct Args {
    preset: Preset,
    interval_us: Option<u64>,
    seconds: f64,
    rounds: u32,
    json: bool,
    output: Option<PathBuf>,
}

impl Default for Args {
    fn default() -> Self {
        Self {
            preset: Preset::Balanced,
            interval_us: None,
            seconds: 3.0,
            rounds: 3,
            json: false,
            output: None,
        }
    }
}

fn main() {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("error: {message}\n");
            print_help();
            std::process::exit(2);
        }
    };
    let interval_us = args
        .interval_us
        .unwrap_or_else(|| args.preset.config().interval_us);
    let report = run_comparison(args.preset, interval_us, args.seconds, args.rounds);
    let json = report.to_json();

    if let Some(path) = args.output.as_ref() {
        if let Err(error) = std::fs::write(path, &json) {
            eprintln!("failed to write {}: {error}", path.display());
            std::process::exit(1);
        }
    }

    if args.json {
        println!("{json}");
    } else {
        println!(
            "Hermes Engine {} — scheduler wake-up A/B",
            hermes_engine::VERSION
        );
        println!(
            "platform={} preset={} interval={} us rounds={} duration/run={:.2} s",
            report.platform,
            report.preset.short_label(),
            report.interval_us,
            report.rounds,
            report.seconds_per_run
        );
        println!(
            "timer request: {}",
            if report.timer_request_active {
                "active"
            } else {
                "inactive / not applicable"
            }
        );
        print_result(&report.sleep_only);
        print_result(&report.adaptive);
        for round in &report.round_results {
            println!(
                "round {} ({} first): p99 sleep={:.2} us adaptive={:.2} us change={:+.2}%",
                round.round,
                round.first_strategy,
                round.sleep_p99_us,
                round.adaptive_p99_us,
                round.p99_improvement_pct,
            );
        }
        println!(
            "improvement: p50={:+.2}% p99={:+.2}% p99.9={:+.2}% max={:+.2}%",
            report.p50_improvement_pct(),
            report.p99_improvement_pct(),
            report.p999_improvement_pct(),
            report.max_improvement_pct()
        );
        println!(
            "p99 consistency: adaptive won {}/{} rounds",
            report.adaptive_p99_wins(),
            report.round_results.len()
        );
        println!(
            "Scope: these are Hermes worker wake-up errors, not input, DPC, network, or electrical latency."
        );
        if let Some(path) = args.output.as_ref() {
            println!("JSON written to {}", path.display());
        }
    }
}

fn print_result(result: &BenchmarkResult) {
    println!(
        "{:<10} n={:<7} p50={:>8.2} us p95={:>8.2} us p99={:>8.2} us p99.9={:>8.2} us max={:>9.2} us missed={} spin={:.2}%",
        result.strategy,
        result.samples,
        result.p50_us,
        result.p95_us,
        result.p99_us,
        result.p999_us,
        result.max_us,
        result.missed_deadlines,
        result.spin_duty_pct,
    );
}

fn parse_args<I>(mut arguments: I) -> Result<Args, String>
where
    I: Iterator<Item = String>,
{
    let mut parsed = Args::default();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--preset" => {
                let value = arguments.next().ok_or("--preset needs a value")?;
                parsed.preset = match value.to_ascii_lowercase().as_str() {
                    "eco" => Preset::Eco,
                    "balanced" => Preset::Balanced,
                    "precision" => Preset::Precision,
                    _ => return Err(format!("unknown preset: {value}")),
                };
            }
            "--interval-us" => {
                parsed.interval_us = Some(parse_next(&mut arguments, "--interval-us")?);
            }
            "--seconds" => parsed.seconds = parse_next(&mut arguments, "--seconds")?,
            "--rounds" => parsed.rounds = parse_next(&mut arguments, "--rounds")?,
            "--output" => {
                parsed.output = Some(PathBuf::from(
                    arguments.next().ok_or("--output needs a path")?,
                ));
            }
            "--json" => parsed.json = true,
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {argument}")),
        }
    }
    Ok(parsed)
}

fn parse_next<T, I>(arguments: &mut I, flag: &str) -> Result<T, String>
where
    T: std::str::FromStr,
    T::Err: std::fmt::Display,
    I: Iterator<Item = String>,
{
    arguments
        .next()
        .ok_or_else(|| format!("{flag} needs a value"))?
        .parse::<T>()
        .map_err(|error| format!("invalid {flag}: {error}"))
}

fn print_help() {
    println!(
        "Hermes scheduler wake-up benchmark\n\n\
         Usage: hermes-bench [options]\n\n\
         Options:\n\
           --preset eco|balanced|precision  Controller profile (default: balanced)\n\
           --interval-us N                  Override interval in microseconds\n\
           --seconds N                      Seconds per strategy per round (default: 3)\n\
           --rounds N                       Alternating A/B rounds (default: 3)\n\
           --json                           Print machine-readable JSON\n\
           --output PATH                    Also save JSON to a file\n\
           -h, --help                       Show this help"
    );
}
