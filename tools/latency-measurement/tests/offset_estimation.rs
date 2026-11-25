//! End-to-end checks for the latency measurement library and CLI.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use sonimesh_latency_measurement::{
    OffsetConfig, OffsetError, SampleFormat, estimate_offset_ms, read_mono, test_signal, write_mono,
};

const SAMPLE_RATE: u32 = 48_000;

/// Builds a recording that contains `reference` shifted `delay_samples` into
/// the future, with leading silence.
fn delayed_recording(delay_samples: usize, reference: &[f32]) -> Vec<f32> {
    let mut recording = vec![0.0; delay_samples];
    recording.extend_from_slice(reference);
    recording
}

/// Returns a unique temporary directory for one test so parallel tests never
/// share files.
fn test_dir(label: &str) -> std::path::PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "sonimesh-latency-e2e-{}-{}-{label}",
        std::process::id(),
        unique
    ))
}

/// Deterministic additive noise in the approximate `[-1.0, 1.0]` range.
///
/// The 16-bit value is exactly representable as `f32`, so the cast is lossless.
#[allow(clippy::cast_precision_loss)]
fn next_noise(state: &mut u32) -> f32 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    let value = (*state >> 8) & 0xFFFF;
    (value as f32 / 32_768.0) - 1.0
}

#[test]
fn synthetic_integer_delay_is_recovered() {
    let reference = test_signal(SAMPLE_RATE, 3, 7);
    let recording = delayed_recording(12_000, &reference);
    let config = OffsetConfig::new(SAMPLE_RATE);
    let estimate = estimate_offset_ms(&config, &reference, &recording).expect("offset found");
    assert!(
        (estimate.offset_ms - 250.0).abs() < 0.1,
        "offset_ms={}",
        estimate.offset_ms
    );
    assert!(
        (estimate.lag_samples - 12_000.0).abs() < 1.0,
        "lag_samples={}",
        estimate.lag_samples
    );
}

#[test]
fn delay_with_attenuation_and_noise_is_recovered() {
    let reference = test_signal(SAMPLE_RATE, 3, 11);
    let mut recording = delayed_recording(7_500, &reference);
    let mut state = 0x1234_5678_u32;
    for sample in &mut recording {
        *sample = *sample * 0.5 + next_noise(&mut state) * 0.05;
    }
    let config = OffsetConfig::new(SAMPLE_RATE);
    let estimate = estimate_offset_ms(&config, &reference, &recording).expect("offset found");
    assert!(
        (estimate.offset_ms - 156.25).abs() < 0.1,
        "offset_ms={}",
        estimate.offset_ms
    );
}

/// A strong echo far outside the fine window but inside the search window
/// competes for the distinctness baseline: the estimate is rejected instead of
/// silently reporting the stronger copy as the offset.
#[test]
fn strong_far_echo_within_search_window_is_rejected() {
    let reference = test_signal(SAMPLE_RATE, 3, 7);
    let mut recording = delayed_recording(5_000, &reference);
    // A second copy at 0.8 amplitude, 2000 samples (about 42 ms) later; the
    // echo sits well outside the +/-32-sample fine window around the peak.
    for (index, sample) in recording.iter_mut().enumerate().skip(7_000) {
        if index - 7_000 < reference.len() {
            *sample += 0.8 * reference[index - 7_000];
        }
    }
    let config = OffsetConfig::new(SAMPLE_RATE);
    let error = estimate_offset_ms(&config, &reference, &recording).expect_err("echo is ambiguous");
    match error {
        OffsetError::NoDistinctPeak {
            best_lag_samples,
            best_score,
            second_best_score,
        } => {
            assert!(
                best_lag_samples.abs_diff(5_000) < 64,
                "best lag {best_lag_samples}"
            );
            assert!(best_score > 0.0, "peak score is positive");
            assert!(
                best_score / second_best_score < config.distinctness_ratio,
                "the echo must compete for the baseline"
            );
        }
        other => panic!("expected NoDistinctPeak, got {other}"),
    }
}

/// A weak far echo stays below the distinctness ratio and the estimate is
/// still accepted at the direct-path lag.
#[test]
fn weak_far_echo_still_accepts_the_direct_lag() {
    let reference = test_signal(SAMPLE_RATE, 3, 7);
    let mut recording = delayed_recording(5_000, &reference);
    for (index, sample) in recording.iter_mut().enumerate().skip(7_000) {
        if index - 7_000 < reference.len() {
            *sample += 0.2 * reference[index - 7_000];
        }
    }
    let config = OffsetConfig::new(SAMPLE_RATE);
    let estimate =
        estimate_offset_ms(&config, &reference, &recording).expect("weak echo is accepted");
    assert!(
        (estimate.lag_samples - 5_000.0).abs() < 1.0,
        "lag_samples={}",
        estimate.lag_samples
    );
    assert!(
        estimate.distinctness_ratio >= config.distinctness_ratio,
        "distinctness meets the configured threshold"
    );
}

#[test]
fn silent_recording_is_rejected() {
    let reference = test_signal(SAMPLE_RATE, 3, 7);
    let recording = vec![0.0; 96_000];
    let config = OffsetConfig::new(SAMPLE_RATE);
    let error =
        estimate_offset_ms(&config, &reference, &recording).expect_err("silence has no peak");
    assert!(matches!(error, OffsetError::NoDistinctPeak { .. }));
}

#[test]
fn recording_shorter_than_probe_is_rejected() {
    let reference = test_signal(SAMPLE_RATE, 3, 7);
    let recording = vec![0.0; 1_000];
    let config = OffsetConfig::new(SAMPLE_RATE);
    let error = estimate_offset_ms(&config, &reference, &recording).expect_err("too short");
    assert!(matches!(error, OffsetError::RecordingTooShort { .. }));
}

#[test]
fn empty_search_window_is_rejected() {
    let reference = test_signal(SAMPLE_RATE, 3, 7);
    let recording = delayed_recording(100, &reference);
    let mut config = OffsetConfig::new(SAMPLE_RATE);
    config.max_search_ms = 0.0;
    let error = estimate_offset_ms(&config, &reference, &recording).expect_err("no search window");
    assert!(matches!(error, OffsetError::EmptySearchWindow));
}

#[test]
fn cli_selects_recording_channel_and_checks_each_file() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir("recording_channel");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let reference_path = dir.join("reference.wav");
    let stereo_path = dir.join("stereo.wav");
    let mono_path = dir.join("mono.wav");
    let reference = test_signal(SAMPLE_RATE, 3, 7);
    let recording = delayed_recording(12_000, &reference);
    write_mono(
        &reference_path,
        SAMPLE_RATE,
        &reference,
        SampleFormat::Float32,
    )
    .unwrap();
    write_mono(&mono_path, SAMPLE_RATE, &recording, SampleFormat::Float32).unwrap();
    // Build interleaved float channels with equal and opposite polarity.
    let mut bytes =
        sonimesh_latency_measurement::encode_mono(SAMPLE_RATE, &[], SampleFormat::Float32);
    let data_size = u32::try_from(recording.len() * 8).unwrap();
    bytes[4..8].copy_from_slice(&(36 + data_size).to_le_bytes());
    bytes[22..24].copy_from_slice(&2_u16.to_le_bytes());
    bytes[28..32].copy_from_slice(&(SAMPLE_RATE * 8).to_le_bytes());
    bytes[32..34].copy_from_slice(&8_u16.to_le_bytes());
    bytes[40..44].copy_from_slice(&data_size.to_le_bytes());
    for sample in recording {
        bytes.extend_from_slice(&(-sample).to_le_bytes());
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(&stereo_path, bytes).unwrap();
    let run = |options: &[&str], recordings: &[&std::path::Path]| {
        Command::new(bin)
            .arg("measure")
            .args(options)
            .arg(&reference_path)
            .args(recordings)
            .output()
            .expect("measure runs")
    };
    let averaged = run(&[], &[&stereo_path]);
    assert!(!averaged.status.success(), "opposite channels cancel");
    let selected = run(
        &["--recording-channel", "2", "--offsets-only"],
        &[&stereo_path],
    );
    assert!(
        selected.status.success(),
        "{}",
        String::from_utf8_lossy(&selected.stderr)
    );
    let offset: f64 = String::from_utf8(selected.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!((offset - 250.0).abs() < 0.1);
    let invalid = run(&["--recording-channel", "2"], &[&stereo_path, &mono_path]);
    let error = String::from_utf8_lossy(&invalid.stderr);
    assert!(!invalid.status.success());
    assert!(
        error.contains("mono.wav") && error.contains("channel 2") && error.contains("1 channel"),
        "{error}"
    );
    let zero = run(&["--recording-channel", "0"], &[&stereo_path]);
    assert!(!zero.status.success());
    assert!(String::from_utf8_lossy(&zero.stderr).contains("must be at least 1"));
    std::fs::remove_dir_all(&dir).expect("cleanup temp dir");
}

#[test]
fn cli_measures_two_channels_from_one_common_interface_recording() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir("common_interface");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("capture.wav");
    let reference = test_signal(SAMPLE_RATE, 3, 7);
    let recording = delayed_recording(12_000, &reference);
    let mut bytes =
        sonimesh_latency_measurement::encode_mono(SAMPLE_RATE, &[], SampleFormat::Float32);
    let data_size = u32::try_from(recording.len() * 8).unwrap();
    bytes[4..8].copy_from_slice(&(36 + data_size).to_le_bytes());
    bytes[22..24].copy_from_slice(&2_u16.to_le_bytes());
    bytes[28..32].copy_from_slice(&(SAMPLE_RATE * 8).to_le_bytes());
    bytes[32..34].copy_from_slice(&8_u16.to_le_bytes());
    bytes[40..44].copy_from_slice(&data_size.to_le_bytes());
    for (index, sample) in recording.iter().enumerate() {
        bytes.extend_from_slice(&reference.get(index).copied().unwrap_or(0.0).to_le_bytes());
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(&path, bytes).unwrap();
    let run = |reference_channel: &str| {
        Command::new(bin)
            .args([
                "measure",
                "--reference-channel",
                reference_channel,
                "--recording-channel",
                "2",
                "--offsets-only",
            ])
            .arg(&path)
            .arg(&path)
            .output()
            .unwrap()
    };
    let result = run("1");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let offset: f64 = String::from_utf8(result.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert!((offset - 250.0).abs() < 0.1);
    let invalid = run("3");
    let error = String::from_utf8_lossy(&invalid.stderr);
    assert!(!invalid.status.success());
    assert!(
        error.contains("reference")
            && error.contains("capture.wav")
            && error.contains("channel 3")
            && error.contains("2 channel"),
        "{error}"
    );
    let zero = run("0");
    assert!(!zero.status.success());
    assert!(
        String::from_utf8_lossy(&zero.stderr).contains("--reference-channel must be at least 1")
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn cli_generate_and_measure_round_trip() {
    for format in [
        SampleFormat::Pcm16,
        SampleFormat::Pcm24,
        SampleFormat::Pcm32,
        SampleFormat::Float32,
        SampleFormat::Float64,
    ] {
        cli_generate_and_measure_round_trip_format(format);
    }
}

fn cli_generate_and_measure_round_trip_format(format: SampleFormat) {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir(&format!("round_trip_{}", format.name()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let reference_path = dir.join("reference.wav");
    let recording_path = dir.join("recording.wav");

    let output = Command::new(bin)
        .args([
            "generate",
            "--sample-rate",
            "48000",
            "--seconds",
            "3",
            "--seed",
            "7",
            "--bits",
            format.name(),
        ])
        .arg(&reference_path)
        .output()
        .expect("generate runs");
    assert!(
        output.status.success(),
        "generate ({}) failed: {}",
        format.name(),
        String::from_utf8_lossy(&output.stderr)
    );

    let reference = read_mono(&reference_path).expect("reference reads");
    let recording = delayed_recording(12_000, &reference.samples);
    write_mono(&recording_path, SAMPLE_RATE, &recording, format).expect("recording writes");

    let output = Command::new(bin)
        .args(["measure", "--max-search-ms", "1000"])
        .arg(&reference_path)
        .arg(&recording_path)
        .output()
        .expect("measure runs");
    assert!(
        output.status.success(),
        "measure ({}) failed: {}",
        format.name(),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let offset_ms = stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("run=1 ").and_then(|rest| {
                rest.split_whitespace()
                    .find_map(|token| token.strip_prefix("offset_ms="))
            })
        })
        .expect("offset line present")
        .parse::<f64>()
        .expect("offset parses");
    assert!(
        (offset_ms - 250.0).abs() < 1.0,
        "format {} offset_ms={offset_ms}",
        format.name()
    );
    assert!(
        stdout.contains("p50_ms=") && stdout.contains("p99_ms="),
        "summary line missing: {stdout}"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup temp dir");
}

#[test]
fn cli_measure_recovers_delay_from_24_bit_recording() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir("24bit");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let reference_path = dir.join("reference.wav");
    let recording_path = dir.join("recording24.wav");

    let output = Command::new(bin)
        .args([
            "generate",
            "--sample-rate",
            "48000",
            "--seconds",
            "3",
            "--seed",
            "7",
        ])
        .arg(&reference_path)
        .output()
        .expect("generate runs");
    assert!(
        output.status.success(),
        "generate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let reference = read_mono(&reference_path).expect("reference reads");
    // 100 ms of leading silence at 48 kHz, stored as 24-bit PCM.
    let delay_samples = 4_800_usize;
    let recording = delayed_recording(delay_samples, &reference.samples);
    write_mono(
        &recording_path,
        SAMPLE_RATE,
        &recording,
        SampleFormat::Pcm24,
    )
    .expect("24-bit recording writes");

    let output = Command::new(bin)
        .args(["measure", "--max-search-ms", "1000"])
        .arg(&reference_path)
        .arg(&recording_path)
        .output()
        .expect("measure runs");
    assert!(
        output.status.success(),
        "measure failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let offset_ms = stdout
        .lines()
        .find_map(|line| {
            line.strip_prefix("run=1 ").and_then(|rest| {
                rest.split_whitespace()
                    .find_map(|token| token.strip_prefix("offset_ms="))
            })
        })
        .expect("offset line present")
        .parse::<f64>()
        .expect("offset parses");
    assert!((offset_ms - 100.0).abs() < 1.0, "offset_ms={offset_ms}");

    std::fs::remove_dir_all(&dir).expect("cleanup temp dir");
}

#[test]
fn cli_measure_rejects_sample_rate_mismatch() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir("mismatch");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let reference_path = dir.join("reference.wav");
    let recording_path = dir.join("recording.wav");

    let output = Command::new(bin)
        .args([
            "generate",
            "--sample-rate",
            "48000",
            "--seconds",
            "1",
            "--seed",
            "7",
        ])
        .arg(&reference_path)
        .output()
        .expect("generate runs");
    assert!(output.status.success(), "generate failed");

    let recording = test_signal(44_100, 1, 7);
    write_mono(&recording_path, 44_100, &recording, SampleFormat::Pcm16).expect("recording writes");

    let output = Command::new(bin)
        .args(["measure"])
        .arg(&reference_path)
        .arg(&recording_path)
        .output()
        .expect("measure runs");
    assert!(!output.status.success(), "mismatch must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("sample rate mismatch"),
        "unexpected stderr: {stderr}"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup temp dir");
}

/// A missing recording fails `measure` fast with an I/O error naming the
/// path, so a broken measurement run cannot silently pass as a successful one.
#[test]
fn cli_measure_fails_fast_on_missing_recording() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir("missing-recording");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let reference_path = dir.join("reference.wav");

    let output = Command::new(bin)
        .args([
            "generate",
            "--sample-rate",
            "48000",
            "--seconds",
            "1",
            "--seed",
            "7",
        ])
        .arg(&reference_path)
        .output()
        .expect("generate runs");
    assert!(output.status.success(), "generate failed");

    let missing_path = dir.join("missing.wav");
    let output = Command::new(bin)
        .args(["measure"])
        .arg(&reference_path)
        .arg(&missing_path)
        .output()
        .expect("measure runs");
    assert!(!output.status.success(), "missing recording must fail");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing.wav") && stderr.contains("wave file I/O failed"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        output.stdout.is_empty(),
        "no per-run output is expected on failure"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup temp dir");
}

#[test]
fn cli_info_reports_declared_wave_properties() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir("info");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let reference_path = dir.join("reference.wav");

    let output = Command::new(bin)
        .args([
            "generate",
            "--sample-rate",
            "48000",
            "--seconds",
            "2",
            "--seed",
            "7",
            "--bits",
            "24",
        ])
        .arg(&reference_path)
        .output()
        .expect("generate runs");
    assert!(
        output.status.success(),
        "generate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new(bin)
        .arg("info")
        .arg(&reference_path)
        .output()
        .expect("info runs");
    assert!(
        output.status.success(),
        "info failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("file=")
            && stdout.contains("sample_rate_hz=48000")
            && stdout.contains("channels=1")
            && stdout.contains("bits_per_sample=24")
            && stdout.contains("audio_format=1")
            && stdout.contains("frames=96000")
            && stdout.contains("duration_ms=2000"),
        "unexpected info output: {stdout}"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup temp dir");
}

#[test]
fn cli_info_reports_per_file_errors_and_continues() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir("info_errors");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let good_path = dir.join("good.wav");
    let missing_path = dir.join("missing.wav");

    let output = Command::new(bin)
        .args(["generate", "--seconds", "1"])
        .arg(&good_path)
        .output()
        .expect("generate runs");
    assert!(
        output.status.success(),
        "generate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = Command::new(bin)
        .arg("info")
        .arg(&good_path)
        .arg(&missing_path)
        .output()
        .expect("info runs");
    assert!(!output.status.success(), "info must fail on a missing file");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("good.wav"),
        "good file info missing: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("missing.wav"),
        "missing-file error missing: {stderr}"
    );
    assert!(
        stderr.contains("failed to inspect 1 of 2 files"),
        "failure summary missing: {stderr}"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup temp dir");
}

#[test]
fn cli_measure_offsets_only_pipes_into_summary() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir("offsets_only");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let reference_path = dir.join("reference.wav");
    let recording_path = dir.join("recording.wav");

    let output = Command::new(bin)
        .args([
            "generate",
            "--sample-rate",
            "48000",
            "--seconds",
            "3",
            "--seed",
            "7",
        ])
        .arg(&reference_path)
        .output()
        .expect("generate runs");
    assert!(
        output.status.success(),
        "generate failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let reference = read_mono(&reference_path).expect("reference reads");
    let recording = delayed_recording(12_000, &reference.samples);
    write_mono(
        &recording_path,
        SAMPLE_RATE,
        &recording,
        SampleFormat::Pcm16,
    )
    .expect("recording writes");

    let output = Command::new(bin)
        .args(["measure", "--offsets-only", "--max-search-ms", "1000"])
        .arg(&reference_path)
        .arg(&recording_path)
        .output()
        .expect("measure runs");
    assert!(
        output.status.success(),
        "measure failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let offsets: Vec<f64> = stdout
        .lines()
        .map(|line| line.trim().parse::<f64>().expect("bare offset line"))
        .collect();
    assert_eq!(offsets.len(), 1, "expected one offset line: {stdout}");
    assert!((offsets[0] - 250.0).abs() < 1.0, "offset_ms={}", offsets[0]);

    // Pipe the bare offsets into the summary subcommand.
    let mut child = Command::new(bin)
        .arg("summary")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("summary spawns");
    child
        .stdin
        .as_mut()
        .expect("stdin piped")
        .write_all(stdout.as_bytes())
        .expect("offset written");
    drop(child.stdin.take());
    let summary_out = child.wait_with_output().expect("summary finishes");
    assert!(
        summary_out.status.success(),
        "summary failed: {}",
        String::from_utf8_lossy(&summary_out.stderr)
    );
    let summary_stdout = String::from_utf8(summary_out.stdout).expect("utf8 summary");
    assert!(
        summary_stdout.contains("runs=1") && summary_stdout.contains("p50_ms="),
        "unexpected summary: {summary_stdout}"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup temp dir");
}

#[test]
fn cli_reports_its_version() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let output = Command::new(bin)
        .arg("--version")
        .output()
        .expect("version runs");
    assert!(
        output.status.success(),
        "version failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.starts_with("sonimesh-latency-measurement ")
            && stdout.contains(env!("CARGO_PKG_VERSION")),
        "unexpected version output: {stdout}"
    );
}

#[test]
fn cli_summary_rejects_empty_stdin() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let mut child = Command::new(bin)
        .arg("summary")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("summary spawns");
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("summary finishes");
    assert!(!output.status.success(), "empty summary must fail closed");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stderr.contains("no offsets provided on stdin"),
        "unexpected stderr: {stderr}"
    );
    assert!(
        String::from_utf8(output.stdout)
            .expect("utf8 stdout")
            .is_empty(),
        "empty summary must not print a summary line"
    );
}

#[test]
fn cli_info_check_gates_on_sample_rate_consistency() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir("info_check");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let reference_path = dir.join("reference.wav");
    let same_rate_path = dir.join("same_rate.wav");
    let other_rate_path = dir.join("other_rate.wav");

    let output = Command::new(bin)
        .args(["generate", "--seconds", "1"])
        .arg(&reference_path)
        .output()
        .expect("generate runs");
    assert!(output.status.success(), "generate failed");
    let output = Command::new(bin)
        .args(["generate", "--seconds", "1"])
        .arg(&same_rate_path)
        .output()
        .expect("generate runs");
    assert!(output.status.success(), "generate failed");
    let output = Command::new(bin)
        .args(["generate", "--sample-rate", "44100", "--seconds", "1"])
        .arg(&other_rate_path)
        .output()
        .expect("generate runs");
    assert!(output.status.success(), "generate failed");

    let output = Command::new(bin)
        .args(["info", "--check"])
        .arg(&reference_path)
        .arg(&same_rate_path)
        .output()
        .expect("info check runs");
    assert!(
        output.status.success(),
        "same-rate check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("check=ok sample_rate_hz=48000"),
        "unexpected check output: {stdout}"
    );

    let output = Command::new(bin)
        .args(["info", "--check"])
        .arg(&reference_path)
        .arg(&other_rate_path)
        .output()
        .expect("info check runs");
    assert!(!output.status.success(), "mismatched check must fail");
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("file="), "per-file info missing: {stdout}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("sample rate mismatch across files"),
        "unexpected stderr: {stderr}"
    );

    std::fs::remove_dir_all(&dir).expect("cleanup temp dir");
}
