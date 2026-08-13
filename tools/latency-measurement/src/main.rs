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

/// SoniMesh control-plane sample-rate bounds the `generate` command accepts.
const MIN_GENERATE_SAMPLE_RATE_HZ: u32 = 8_000;
const MAX_GENERATE_SAMPLE_RATE_HZ: u32 = 192_000;
/// Longest accepted reference duration, in seconds.
const MAX_GENERATE_SECONDS: u32 = 3_600;
/// Upper bound on generated reference samples (10 minutes at 192 kHz), which
/// keeps the in-memory buffer and the written WAVE file bounded.
const MAX_GENERATE_SAMPLES: u32 = MAX_GENERATE_SAMPLE_RATE_HZ * 600;

/// Top-level parsed command.
#[derive(Debug)]
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
#[derive(Debug)]
struct GenerateArgs {
    sample_rate_hz: u32,
    seconds: u32,
    seed: u32,
    output: PathBuf,
}

/// Arguments for the `measure` subcommand.
#[derive(Debug)]
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
    match parse_args(std::env::args().skip(1))? {
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
    summarize_lines(std::io::stdin().lock())
}

fn summarize_lines<R: BufRead>(reader: R) -> Result<(), Box<dyn Error>> {
    let mut offsets_ms = Vec::new();
    for line in reader.lines() {
        let line = line?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let value: f64 = trimmed
            .parse()
            .map_err(|error| format!("invalid offset `{trimmed}`: {error}"))?;
        if !value.is_finite() {
            return Err(format!("invalid offset `{trimmed}`: not a finite number").into());
        }
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

fn parse_args<I>(args: I) -> Result<Parsed, Box<dyn Error>>
where
    I: IntoIterator<Item = String>,
{
    let mut args = args.into_iter();
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
    if !(MIN_GENERATE_SAMPLE_RATE_HZ..=MAX_GENERATE_SAMPLE_RATE_HZ).contains(&sample_rate_hz) {
        return Err(format!(
            "--sample-rate must be in {MIN_GENERATE_SAMPLE_RATE_HZ}..={MAX_GENERATE_SAMPLE_RATE_HZ} Hz"
        ));
    }
    if !(1..=MAX_GENERATE_SECONDS).contains(&seconds) {
        return Err(format!("--seconds must be in 1..={MAX_GENERATE_SECONDS}"));
    }
    if sample_rate_hz.saturating_mul(seconds) > MAX_GENERATE_SAMPLES {
        return Err(format!(
            "--seconds and --sample-rate exceed the {MAX_GENERATE_SAMPLES}-sample reference bound"
        ));
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
    if !max_search_ms.is_finite() || max_search_ms <= 0.0 {
        return Err("--max-search-ms must be positive".into());
    }
    if coarse_factor == 0 {
        return Err("--coarse-factor must be non-zero".into());
    }
    if !distinctness_ratio.is_finite() || distinctness_ratio <= 0.0 {
        return Err("--distinctness-ratio must be positive".into());
    }
    if let Some(probe_ms) = probe_ms {
        if !probe_ms.is_finite() || probe_ms <= 0.0 {
            return Err("--probe-ms must be positive".into());
        }
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

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::PathBuf;

    use super::{
        Parsed, parse_args, parse_f64, parse_generate, parse_measure, parse_u32, parse_usize,
        summarize_lines,
    };

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, observed {actual}"
        );
    }

    #[test]
    fn empty_and_help_parse_to_help() {
        assert!(matches!(parse_args(args(&[])).expect("help"), Parsed::Help));
        assert!(matches!(
            parse_args(args(&["--help"])).expect("help"),
            Parsed::Help
        ));
        assert!(matches!(
            parse_args(args(&["-h"])).expect("help"),
            Parsed::Help
        ));
    }

    #[test]
    fn generate_uses_defaults() {
        let Parsed::Generate(generate) =
            parse_args(args(&["generate", "reference.wav"])).expect("parsed")
        else {
            panic!("expected generate");
        };
        assert_eq!(generate.sample_rate_hz, 48_000);
        assert_eq!(generate.seconds, 10);
        assert_eq!(generate.seed, 7);
        assert_eq!(generate.output, PathBuf::from("reference.wav"));
    }

    #[test]
    fn generate_accepts_all_flags() {
        let Parsed::Generate(generate) = parse_args(args(&[
            "generate",
            "--sample-rate",
            "44100",
            "--seconds",
            "5",
            "--seed",
            "11",
            "ref.wav",
        ]))
        .expect("parsed") else {
            panic!("expected generate");
        };
        assert_eq!(generate.sample_rate_hz, 44_100);
        assert_eq!(generate.seconds, 5);
        assert_eq!(generate.seed, 11);
        assert_eq!(generate.output, PathBuf::from("ref.wav"));
    }

    #[test]
    fn generate_rejects_missing_output_and_extra_paths() {
        let error = parse_args(args(&["generate"])).expect_err("missing output");
        assert!(error.to_string().contains("requires an output path"));

        let error = parse_args(args(&["generate", "a.wav", "b.wav"])).expect_err("extra path");
        assert!(error.to_string().contains("exactly one output path"));
    }

    #[test]
    fn generate_rejects_zero_rate_and_seconds() {
        let error = parse_args(args(&["generate", "--sample-rate", "0", "ref.wav"]))
            .expect_err("zero rate");
        assert!(
            error
                .to_string()
                .contains("--sample-rate must be in 8000..=192000 Hz")
        );

        let error =
            parse_args(args(&["generate", "--seconds", "0", "ref.wav"])).expect_err("zero seconds");
        assert!(error.to_string().contains("--seconds must be in 1..=3600"));
    }

    #[test]
    fn generate_rejects_out_of_range_rate_and_seconds() {
        let error = parse_args(args(&["generate", "--sample-rate", "4000", "ref.wav"]))
            .expect_err("low rate");
        assert!(
            error
                .to_string()
                .contains("--sample-rate must be in 8000..=192000 Hz")
        );

        let error = parse_args(args(&["generate", "--sample-rate", "200000", "ref.wav"]))
            .expect_err("high rate");
        assert!(
            error
                .to_string()
                .contains("--sample-rate must be in 8000..=192000 Hz")
        );

        let error =
            parse_args(args(&["generate", "--seconds", "3601", "ref.wav"])).expect_err("too long");
        assert!(error.to_string().contains("--seconds must be in 1..=3600"));
    }

    #[test]
    fn generate_rejects_excessive_total_samples() {
        let error = parse_args(args(&[
            "generate",
            "--sample-rate",
            "192000",
            "--seconds",
            "601",
            "ref.wav",
        ]))
        .expect_err("too many samples");
        assert!(
            error
                .to_string()
                .contains("115200000-sample reference bound")
        );
    }

    #[test]
    fn generate_rejects_unknown_flag_and_missing_value() {
        let error =
            parse_args(args(&["generate", "--bogus", "ref.wav"])).expect_err("unknown flag");
        assert!(
            error
                .to_string()
                .contains("unknown flag `--bogus` for generate")
        );

        let error = parse_args(args(&["generate", "--seed"])).expect_err("missing value");
        assert!(error.to_string().contains("--seed requires a value"));
    }

    #[test]
    fn generate_help_short_circuits() {
        assert!(matches!(
            parse_args(args(&["generate", "--help", "ref.wav"])).expect("help"),
            Parsed::Help
        ));
    }

    #[test]
    fn measure_uses_defaults() {
        let Parsed::Measure(measure) =
            parse_args(args(&["measure", "reference.wav", "recording.wav"])).expect("parsed")
        else {
            panic!("expected measure");
        };
        assert_close(measure.max_search_ms, 1_000.0);
        assert_eq!(measure.coarse_factor, 16);
        assert_close(measure.distinctness_ratio, 3.0);
        assert!(measure.probe_ms.is_none());
        assert_eq!(measure.reference, PathBuf::from("reference.wav"));
        assert_eq!(measure.recordings, vec![PathBuf::from("recording.wav")]);
    }

    #[test]
    fn measure_accepts_all_flags() {
        let Parsed::Measure(measure) = parse_args(args(&[
            "measure",
            "--max-search-ms",
            "500",
            "--coarse-factor",
            "8",
            "--distinctness-ratio",
            "2.5",
            "--probe-ms",
            "32",
            "reference.wav",
            "a.wav",
            "b.wav",
        ]))
        .expect("parsed") else {
            panic!("expected measure");
        };
        assert_close(measure.max_search_ms, 500.0);
        assert_eq!(measure.coarse_factor, 8);
        assert_close(measure.distinctness_ratio, 2.5);
        let probe = measure.probe_ms.expect("probe configured");
        assert_close(probe, 32.0);
        assert_eq!(measure.reference, PathBuf::from("reference.wav"));
        assert_eq!(
            measure.recordings,
            vec![PathBuf::from("a.wav"), PathBuf::from("b.wav")]
        );
    }

    #[test]
    fn measure_requires_reference_and_recording() {
        let error = parse_args(args(&["measure", "reference.wav"])).expect_err("single path");
        assert!(error.to_string().contains("at least one recording"));

        let error = parse_args(args(&["measure"])).expect_err("no paths");
        assert!(error.to_string().contains("at least one recording"));
    }

    #[test]
    fn measure_rejects_non_positive_bounds() {
        let error = parse_args(args(&["measure", "--max-search-ms", "0", "r.wav", "c.wav"]))
            .expect_err("zero search");
        assert!(
            error
                .to_string()
                .contains("--max-search-ms must be positive")
        );

        let error = parse_args(args(&["measure", "--coarse-factor", "0", "r.wav", "c.wav"]))
            .expect_err("zero coarse");
        assert!(
            error
                .to_string()
                .contains("--coarse-factor must be non-zero")
        );

        let error = parse_args(args(&[
            "measure",
            "--distinctness-ratio",
            "0",
            "r.wav",
            "c.wav",
        ]))
        .expect_err("zero ratio");
        assert!(
            error
                .to_string()
                .contains("--distinctness-ratio must be positive")
        );
    }

    #[test]
    fn measure_rejects_non_finite_bounds_and_probe() {
        for bad in ["NaN", "inf", "-inf"] {
            let error = parse_args(args(&["measure", "--max-search-ms", bad, "r.wav", "c.wav"]))
                .expect_err("non-finite search");
            assert!(
                error
                    .to_string()
                    .contains("--max-search-ms must be positive")
            );

            let error = parse_args(args(&[
                "measure",
                "--distinctness-ratio",
                bad,
                "r.wav",
                "c.wav",
            ]))
            .expect_err("non-finite ratio");
            assert!(
                error
                    .to_string()
                    .contains("--distinctness-ratio must be positive")
            );
        }

        for bad in ["NaN", "inf", "-inf", "0", "-4"] {
            let error = parse_args(args(&["measure", "--probe-ms", bad, "r.wav", "c.wav"]))
                .expect_err("bad probe");
            assert!(error.to_string().contains("--probe-ms must be positive"));
        }
    }

    #[test]
    fn summary_rejects_non_finite_offsets() {
        let error = summarize_lines(Cursor::new("10\nNaN\n".as_bytes())).expect_err("NaN offset");
        assert!(error.to_string().contains("invalid offset `NaN`"));

        let error = summarize_lines(Cursor::new("-inf\n".as_bytes())).expect_err("inf offset");
        assert!(error.to_string().contains("invalid offset `-inf`"));
    }

    #[test]
    fn measure_rejects_unknown_flag_and_help_short_circuits() {
        let error =
            parse_args(args(&["measure", "--bogus", "r.wav", "c.wav"])).expect_err("unknown flag");
        assert!(
            error
                .to_string()
                .contains("unknown flag `--bogus` for measure")
        );

        assert!(matches!(
            parse_args(args(&["measure", "--help"])).expect("help"),
            Parsed::Help
        ));
    }

    #[test]
    fn summary_accepts_only_no_arguments() {
        assert!(matches!(
            parse_args(args(&["summary"])).expect("summary"),
            Parsed::Summary
        ));

        let error = parse_args(args(&["summary", "extra"])).expect_err("extra arg");
        assert!(error.to_string().contains("summary takes no arguments"));
    }

    #[test]
    fn unknown_subcommand_is_rejected() {
        let error = parse_args(args(&["bogus"])).expect_err("unknown command");
        assert!(error.to_string().contains("unknown subcommand `bogus`"));
    }

    #[test]
    fn typed_value_helpers_validate_missing_and_invalid_input() {
        let none: Option<&String> = None;
        assert_eq!(
            parse_u32(none, "--sample-rate").expect_err("missing"),
            "--sample-rate requires a value"
        );
        assert_eq!(
            parse_u32(Some(&"abc".to_owned()), "--sample-rate").expect_err("invalid"),
            "--sample-rate expects an integer: invalid digit found in string"
        );
        assert_eq!(
            parse_u32(Some(&"44100".to_owned()), "--sample-rate").expect("value"),
            44_100
        );
        assert_eq!(
            parse_usize(Some(&"8".to_owned()), "--coarse-factor").expect("value"),
            8
        );
        assert_close(
            parse_f64(Some(&"2.5".to_owned()), "--distinctness-ratio").expect("value"),
            2.5,
        );
    }

    #[test]
    fn sub_parsers_reject_missing_flag_values() {
        assert_eq!(
            parse_generate(&args(&["--seconds"])).expect_err("missing value"),
            "--seconds requires a value"
        );
        assert_eq!(
            parse_measure(&args(&["--probe-ms"])).expect_err("missing value"),
            "--probe-ms requires a value"
        );
    }
}
