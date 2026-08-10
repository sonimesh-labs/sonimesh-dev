//! End-to-end checks for the latency measurement library and CLI.

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use sonimesh_latency_measurement::{
    OffsetConfig, OffsetError, estimate_offset_ms, read_mono, test_signal, write_mono_16bit,
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
fn cli_generate_and_measure_round_trip() {
    let bin = env!("CARGO_BIN_EXE_sonimesh-latency-measurement");
    let dir = test_dir("round_trip");
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
    write_mono_16bit(&recording_path, SAMPLE_RATE, &recording).expect("recording writes");

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
    assert!((offset_ms - 250.0).abs() < 1.0, "offset_ms={offset_ms}");
    assert!(
        stdout.contains("p50_ms=") && stdout.contains("p99_ms="),
        "summary line missing: {stdout}"
    );

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
    write_mono_16bit(&recording_path, 44_100, &recording).expect("recording writes");

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
