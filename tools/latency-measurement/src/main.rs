//! Command-line driver for the SoniMesh latency measurement tool.
//!
//! Subcommands:
//! - `generate` writes a deterministic MLS reference WAVE file;
//! - `measure` estimates the offset between a reference and one or more
//!   recordings, printing per-run offsets and a p50/p95/p99 summary;
//! - `summary` reads one offset per line from stdin and prints the summary.

use std::error::Error;
use std::io::BufRead;
use std::path::PathBuf;
use std::process::ExitCode;

use sonimesh_latency_measurement::{
    OffsetConfig, estimate_offset_ms, read_mono, summarize_ms, test_signal, write_mono_16bit,
};

/// Top-level parsed command.
enum Parsed {
    /// Show usage text.
    Help,
    /// Write a reference WAVE file.
    Generate(GenerateArgs),
    /// Estimate offsets for one or more recordings.
    Measure(MeasureArgs),
    /// Summarize offsets read from stdin.
    Summary,
}

/// Arguments for the `generate` subcommand.
struct GenerateArgs {
    sample_rate_hz: u32,
    seconds: u32,
    seed: u32,
    output: PathBuf,
}

/// Arguments for the `measure` subcommand.
struct MeasureArgs {
    max_search_ms: f64,
    coarse_factor: usize,
    distinctness_ratio: f64,
    probe_ms: Option<f64>,
    reference: PathBuf,
    recordings: Vec<PathBuf>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    match parse_args()? {
        Parsed::Help => print_usage(),
        Parsed::Generate(args) => run_generate(&args)?,
        Parsed::Measure(args) => run_measure(&args)?,
        Parsed::Summary => run_summary()?,
    }
    Ok(())
}

fn run_generate(args: &GenerateArgs) -> Result<(), Box<dyn Error>> {
    let signal = test_signal(args.sample_rate_hz, args.seconds, args.seed);
    write_mono_16bit(&args.output, args.sample_rate_hz, &signal)
        .map_err(|error| format!("write {}: {error}", args.output.display()))?;
    println!(
        "wrote {} sample_rate={}Hz seconds={} seed={} samples={}",
        args.output.display(),
        args.sample_rate_hz,
        args.seconds,
        args.seed,
        signal.len()
    );
    Ok(())
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
fn run_measure(args: &MeasureArgs) -> Result<(), Box<dyn Error>> {
    let reference = read_mono(&args.reference)
        .map_err(|error| format!("reference {}: {error}", args.reference.display()))?;
    let mut offsets_ms = Vec::with_capacity(args.recordings.len());
    for (index, path) in args.recordings.iter().enumerate() {
        let recording =
            read_mono(path).map_err(|error| format!("recording {}: {error}", path.display()))?;
        if recording.sample_rate_hz != reference.sample_rate_hz {
            return Err(format!(
                "sample rate mismatch: reference {} Hz, {} is {} Hz",
                reference.sample_rate_hz,
                path.display(),
                recording.sample_rate_hz
            )
            .into());
        }
        let mut config = OffsetConfig::new(reference.sample_rate_hz);
        config.max_search_ms = args.max_search_ms;
        config.coarse_factor = args.coarse_factor;
        config.distinctness_ratio = args.distinctness_ratio;
        if let Some(probe_ms) = args.probe_ms {
            config.probe_len =
                (probe_ms * f64::from(reference.sample_rate_hz) / 1_000.0).round() as usize;
        }
        let estimate = estimate_offset_ms(&config, &reference.samples, &recording.samples)
            .map_err(|error| format!("recording {}: {error}", path.display()))?;
        println!(
            "run={} recording={} offset_ms={:.3} lag_samples={:.3} distinctness_ratio={:.1}",
            index + 1,
            path.display(),
            estimate.offset_ms,
            estimate.lag_samples,
            estimate.distinctness_ratio
        );
        offsets_ms.push(estimate.offset_ms);
    }
    if let Some(summary) = summarize_ms(&offsets_ms) {
        println!(
            "runs={} min_ms={:.3} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3}",
            offsets_ms.len(),
            summary.min_ms,
            summary.p50_ms,
            summary.p95_ms,
            summary.p99_ms,
            summary.max_ms
        );
    }
    Ok(())
}

fn run_summary() -> Result<(), Box<dyn Error>> {
    let mut offsets_ms = Vec::new();
    for line in std::io::stdin().lock().lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: f64 = trimmed
            .parse()
            .map_err(|error| format!("invalid offset `{trimmed}`: {error}"))?;
        offsets_ms.push(value);
    }
    match summarize_ms(&offsets_ms) {
        Some(summary) => println!(
            "runs={} min_ms={:.3} p50_ms={:.3} p95_ms={:.3} p99_ms={:.3} max_ms={:.3}",
            offsets_ms.len(),
            summary.min_ms,
            summary.p50_ms,
            summary.p95_ms,
            summary.p99_ms,
            summary.max_ms
        ),
        None => eprintln!("summary: no offsets provided on stdin"),
    }
    Ok(())
}

fn parse_args() -> Result<Parsed, Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let Some(first) = args.next() else {
        return Ok(Parsed::Help);
    };
    let rest: Vec<String> = args.collect();
    match first.as_str() {
        "-h" | "--help" => Ok(Parsed::Help),
        "generate" => match parse_generate(&rest)? {
            Some(args) => Ok(Parsed::Generate(args)),
            None => Ok(Parsed::Help),
        },
        "measure" => match parse_measure(&rest)? {
            Some(args) => Ok(Parsed::Measure(args)),
            None => Ok(Parsed::Help),
        },
        "summary" => {
            if rest.is_empty() {
                Ok(Parsed::Summary)
            } else {
                Err("summary takes no arguments".into())
            }
        }
        other => Err(format!("unknown subcommand `{other}`; run with --help").into()),
    }
}

fn parse_generate(rest: &[String]) -> Result<Option<GenerateArgs>, String> {
    if rest.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(None);
    }
    let mut sample_rate_hz = 48_000_u32;
    let mut seconds = 10_u32;
    let mut seed = 7_u32;
    let mut output: Option<PathBuf> = None;
    let mut index = 0;
    while index < rest.len() {
        let arg = &rest[index];
        match arg.as_str() {
            "--sample-rate" => {
                index += 1;
                sample_rate_hz = parse_u32(rest.get(index), "--sample-rate")?;
            }
            "--seconds" => {
                index += 1;
                seconds = parse_u32(rest.get(index), "--seconds")?;
            }
            "--seed" => {
                index += 1;
                seed = parse_u32(rest.get(index), "--seed")?;
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown flag `{flag}` for generate"));
            }
            _ => {
                if output.replace(PathBuf::from(arg)).is_some() {
                    return Err("generate accepts exactly one output path".into());
                }
            }
        }
        index += 1;
    }
    if sample_rate_hz == 0 {
        return Err("--sample-rate must be non-zero".into());
    }
    if seconds == 0 {
        return Err("--seconds must be non-zero".into());
    }
    let output = output.ok_or_else(|| "generate requires an output path".to_owned())?;
    Ok(Some(GenerateArgs {
        sample_rate_hz,
        seconds,
        seed,
        output,
    }))
}

fn parse_measure(rest: &[String]) -> Result<Option<MeasureArgs>, String> {
    if rest.iter().any(|arg| arg == "-h" || arg == "--help") {
        return Ok(None);
    }
    let mut max_search_ms = 1_000.0_f64;
    let mut coarse_factor = 16_usize;
    let mut distinctness_ratio = 3.0_f64;
    let mut probe_ms: Option<f64> = None;
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut index = 0;
    while index < rest.len() {
        let arg = &rest[index];
        match arg.as_str() {
            "--max-search-ms" => {
                index += 1;
                max_search_ms = parse_f64(rest.get(index), "--max-search-ms")?;
            }
            "--coarse-factor" => {
                index += 1;
                coarse_factor = parse_usize(rest.get(index), "--coarse-factor")?;
            }
            "--distinctness-ratio" => {
                index += 1;
                distinctness_ratio = parse_f64(rest.get(index), "--distinctness-ratio")?;
            }
            "--probe-ms" => {
                index += 1;
                probe_ms = Some(parse_f64(rest.get(index), "--probe-ms")?);
            }
            flag if flag.starts_with('-') => {
                return Err(format!("unknown flag `{flag}` for measure"));
            }
            _ => paths.push(PathBuf::from(arg)),
        }
        index += 1;
    }
    if max_search_ms <= 0.0 {
        return Err("--max-search-ms must be positive".into());
    }
    if coarse_factor == 0 {
        return Err("--coarse-factor must be non-zero".into());
    }
    if distinctness_ratio <= 0.0 {
        return Err("--distinctness-ratio must be positive".into());
    }
    if paths.len() < 2 {
        return Err("measure requires a reference and at least one recording".into());
    }
    let mut paths = paths.into_iter();
    let reference = paths.next().expect("length checked above");
    let recordings = paths.collect();
    Ok(Some(MeasureArgs {
        max_search_ms,
        coarse_factor,
        distinctness_ratio,
        probe_ms,
        reference,
        recordings,
    }))
}

fn parse_u32(value: Option<&String>, flag: &str) -> Result<u32, String> {
    let value = value.ok_or_else(|| format!("{flag} requires a value"))?;
    value
        .parse()
        .map_err(|error| format!("{flag} expects an integer: {error}"))
}

fn parse_usize(value: Option<&String>, flag: &str) -> Result<usize, String> {
    let value = value.ok_or_else(|| format!("{flag} requires a value"))?;
    value
        .parse()
        .map_err(|error| format!("{flag} expects an integer: {error}"))
}

fn parse_f64(value: Option<&String>, flag: &str) -> Result<f64, String> {
    let value = value.ok_or_else(|| format!("{flag} requires a value"))?;
    value
        .parse()
        .map_err(|error| format!("{flag} expects a number: {error}"))
}

fn print_usage() {
    println!(
        r"SoniMesh latency measurement tool

Usage:
  sonimesh-latency-measurement generate [--sample-rate N] [--seconds N] [--seed N] <output.wav>
  sonimesh-latency-measurement measure [--max-search-ms N] [--coarse-factor N] [--distinctness-ratio R] [--probe-ms N] <reference.wav> <recording.wav>...
  sonimesh-latency-measurement summary
  sonimesh-latency-measurement --help

Commands:
  generate  Write a deterministic MLS reference WAVE file.
  measure   Estimate the offset between a reference and one or more recordings.
  summary   Summarize offsets read from stdin, one number per line.

Options:
  --sample-rate N         Reference sample rate in Hz (default: 48000).
  --seconds N             Reference length in seconds (default: 10).
  --seed N                MLS generator seed (default: 7).
  --max-search-ms N       Search window after the recording start, in ms (default: 1000).
  --coarse-factor N       Coarse-search downsampling factor (default: 16).
  --distinctness-ratio R  Minimum best/second-best peak ratio (default: 3.0).
  --probe-ms N            Correlation probe duration in ms (default: 32768 samples)."
    );
}
