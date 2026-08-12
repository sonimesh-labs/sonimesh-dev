//! Deterministic property coverage for the platform-neutral output boundary.
//!
//! The platform crate publishes two hardware-free contracts: bounded output
//! configuration validation and diagnostic-safe `Debug` redaction. This test
//! drives randomly generated `OutputConfig` values against an independent
//! predicate of the documented validation priority order (sample rate,
//! channels, maximum callback, requested callback, interleaved overflow) and
//! asserts that randomized device metadata and free-form backend messages
//! never leak through `Debug` output while remaining available through the
//! user-facing `Display` surface.

use sonimesh_platform::{
    MAX_CALLBACK_FRAMES, MAX_OUTPUT_CHANNELS, MAX_OUTPUT_SAMPLE_RATE_HZ, MIN_CALLBACK_FRAMES,
    MIN_OUTPUT_SAMPLE_RATE_HZ, OutputConfig, OutputConfigError, OutputDeviceId, OutputDeviceInfo,
    OutputError, RenderSourceFactoryError, SampleFormat,
};

/// Small deterministic xorshift64 generator; no external dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }

    fn pick<'a, T>(&mut self, choices: &'a [T]) -> &'a T {
        &choices[self.next_usize(choices.len())]
    }
}

/// Independent predicate of the documented validation priority order.
///
/// The first violated constraint in this exact order is the error a real
/// `OutputConfig::validate()` call must report: sample rate, channel count,
/// maximum callback frames, requested callback range, requested exceeding
/// maximum, then the interleaved-sample overflow guard.
fn expected_validation_error(config: &OutputConfig) -> Option<OutputConfigError> {
    if !(MIN_OUTPUT_SAMPLE_RATE_HZ..=MAX_OUTPUT_SAMPLE_RATE_HZ).contains(&config.sample_rate_hz) {
        return Some(OutputConfigError::InvalidSampleRate);
    }
    if !(1..=MAX_OUTPUT_CHANNELS).contains(&config.channels) {
        return Some(OutputConfigError::InvalidChannelCount);
    }
    if !(MIN_CALLBACK_FRAMES..=MAX_CALLBACK_FRAMES).contains(&config.maximum_callback_frames) {
        return Some(OutputConfigError::InvalidMaximumCallbackFrames);
    }
    if let Some(requested) = config.requested_callback_frames {
        if !(MIN_CALLBACK_FRAMES..=MAX_CALLBACK_FRAMES).contains(&requested) {
            return Some(OutputConfigError::InvalidRequestedCallbackFrames);
        }
        if requested > config.maximum_callback_frames {
            return Some(OutputConfigError::RequestedExceedsMaximum);
        }
    }
    let fits = usize::try_from(config.maximum_callback_frames)
        .ok()
        .and_then(|frames| frames.checked_mul(usize::from(config.channels)));
    if fits.is_none() {
        return Some(OutputConfigError::InterleavedSizeOverflow);
    }
    None
}

/// Returns a boundary or fully random value, biased toward the curated edges
/// so every validation branch is reachable within a bounded walk.
fn pick_u32(rng: &mut XorShift64, curated: &[u32]) -> u32 {
    if rng.next_usize(2) == 0 {
        *rng.pick(curated)
    } else {
        rng.next_u32()
    }
}

fn pick_u16(rng: &mut XorShift64, curated: &[u16]) -> u16 {
    if rng.next_usize(2) == 0 {
        *rng.pick(curated)
    } else {
        u16::try_from(rng.next_u32() % 65_536).expect("modulo keeps the value in u16")
    }
}

fn random_config(rng: &mut XorShift64) -> OutputConfig {
    let sample_rate_hz = pick_u32(
        rng,
        &[
            0,
            7_999,
            8_000,
            8_001,
            44_100,
            48_000,
            96_000,
            192_000,
            192_001,
            u32::MAX,
        ],
    );
    let channels = pick_u16(rng, &[0, 1, 2, 8, 9, u16::MAX]);
    let maximum_callback_frames = pick_u32(rng, &[0, 15, 16, 17, 480, 960, 8_192, 8_193, u32::MAX]);
    let requested_callback_frames = if rng.next_usize(3) == 0 {
        None
    } else {
        Some(pick_u32(
            rng,
            &[0, 15, 16, 17, 480, 960, 8_192, 8_193, u32::MAX],
        ))
    };
    let sample_format = *rng.pick(&[SampleFormat::Float32, SampleFormat::Signed16]);
    OutputConfig {
        sample_rate_hz,
        channels,
        sample_format,
        requested_callback_frames,
        maximum_callback_frames,
    }
}

/// Covers every reachable validation branch across the random walk.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ValidationCoverage(u8);

impl ValidationCoverage {
    const VALID: u8 = 1 << 0;
    const SAMPLE_RATE: u8 = 1 << 1;
    const CHANNELS: u8 = 1 << 2;
    const MAX_CALLBACK: u8 = 1 << 3;
    const REQUESTED_RANGE: u8 = 1 << 4;
    const REQUESTED_EXCEEDS: u8 = 1 << 5;

    fn mark(&mut self, flag: u8) {
        self.0 |= flag;
    }

    fn covered(self, flag: u8) -> bool {
        self.0 & flag != 0
    }
}

/// Every reachable output configuration branch must match the independent
/// validation predicate, and valid configs must report an exact interleaved
/// sample count equal to `maximum_callback_frames * channels`.
#[test]
fn randomized_output_configs_match_an_independent_validation_model() {
    const STEPS: usize = 20_000;
    let mut rng = XorShift64(0x504C_0000_2026_0010);
    let mut coverage = ValidationCoverage::default();

    for _ in 0..STEPS {
        let config = random_config(&mut rng);
        match expected_validation_error(&config) {
            None => {
                coverage.mark(ValidationCoverage::VALID);
                assert_eq!(config.validate(), Ok(()), "config: {config:?}");
                let frames = usize::try_from(config.maximum_callback_frames)
                    .expect("valid maximum fits usize");
                let expected = frames
                    .checked_mul(usize::from(config.channels))
                    .expect("valid config never overflows interleaved samples");
                assert_eq!(
                    config.interleaved_samples(),
                    Ok(expected),
                    "config: {config:?}"
                );
            }
            Some(OutputConfigError::InvalidSampleRate) => {
                coverage.mark(ValidationCoverage::SAMPLE_RATE);
                assert_eq!(
                    config.validate(),
                    Err(OutputConfigError::InvalidSampleRate),
                    "config: {config:?}"
                );
            }
            Some(OutputConfigError::InvalidChannelCount) => {
                coverage.mark(ValidationCoverage::CHANNELS);
                assert_eq!(
                    config.validate(),
                    Err(OutputConfigError::InvalidChannelCount),
                    "config: {config:?}"
                );
            }
            Some(OutputConfigError::InvalidMaximumCallbackFrames) => {
                coverage.mark(ValidationCoverage::MAX_CALLBACK);
                assert_eq!(
                    config.validate(),
                    Err(OutputConfigError::InvalidMaximumCallbackFrames),
                    "config: {config:?}"
                );
            }
            Some(OutputConfigError::InvalidRequestedCallbackFrames) => {
                coverage.mark(ValidationCoverage::REQUESTED_RANGE);
                assert_eq!(
                    config.validate(),
                    Err(OutputConfigError::InvalidRequestedCallbackFrames),
                    "config: {config:?}"
                );
            }
            Some(OutputConfigError::RequestedExceedsMaximum) => {
                coverage.mark(ValidationCoverage::REQUESTED_EXCEEDS);
                assert_eq!(
                    config.validate(),
                    Err(OutputConfigError::RequestedExceedsMaximum),
                    "config: {config:?}"
                );
            }
            Some(OutputConfigError::InterleavedSizeOverflow) => {
                // Only reachable when `usize` cannot hold the product, which
                // cannot happen on 64-bit hosts with u16 channels and u32
                // frames; the predicate keeps the documented priority order.
                assert_eq!(
                    config.validate(),
                    Err(OutputConfigError::InterleavedSizeOverflow),
                    "config: {config:?}"
                );
                assert_eq!(
                    config.interleaved_samples(),
                    Err(OutputConfigError::InterleavedSizeOverflow),
                    "config: {config:?}"
                );
            }
        }
    }

    assert!(
        coverage.covered(ValidationCoverage::VALID),
        "walk must accept bounded configs"
    );
    assert!(
        coverage.covered(ValidationCoverage::SAMPLE_RATE),
        "walk must reject out-of-range sample rates"
    );
    assert!(
        coverage.covered(ValidationCoverage::CHANNELS),
        "walk must reject out-of-range channel counts"
    );
    assert!(
        coverage.covered(ValidationCoverage::MAX_CALLBACK),
        "walk must reject out-of-range maximum callback frames"
    );
    assert!(
        coverage.covered(ValidationCoverage::REQUESTED_RANGE),
        "walk must reject out-of-range requested callback frames"
    );
    assert!(
        coverage.covered(ValidationCoverage::REQUESTED_EXCEEDS),
        "walk must reject a requested callback above the maximum"
    );
}

/// Characters chosen to stress redaction: quotes, braces, whitespace, control
/// characters, CJK, an emoji, and a zero-width joiner.
const REDACTION_CHARACTERS: &[char] = &[
    'a', 'Z', '0', ' ', '\t', '\n', '\r', '{', '}', '"', '\'', '\\', '[', ']', '(', ')', ',', ':',
    '中', '😀', '\u{0}', '\u{7f}', '\u{200d}', 'R', 'E', 'D', 'A', 'C', 'T', 'E', 'D',
];

/// Builds a value with a unique marker prefix and returns `(value, marker)`.
fn random_value(rng: &mut XorShift64) -> (String, String) {
    let marker = format!("LEAK-{:08x}", rng.next_u32());
    let length = 1 + rng.next_usize(24);
    let mut value = marker.clone();
    for _ in 0..length {
        value.push(REDACTION_CHARACTERS[rng.next_usize(REDACTION_CHARACTERS.len())]);
    }
    (value, marker)
}

/// Randomized device metadata must never appear in `Debug` output: the
/// persistent id renders as a constant redacted marker and the display name
/// renders only as a presence flag, regardless of the raw content.
#[test]
fn randomized_device_metadata_never_leaks_through_debug() {
    const STEPS: usize = 5_000;
    let mut rng = XorShift64(0x504C_0000_2026_0011);

    for _ in 0..STEPS {
        let (id_value, id_marker) = random_value(&mut rng);
        let (name_value, name_marker) = random_value(&mut rng);
        let is_default = rng.next_usize(2) == 0;

        let id = OutputDeviceId::new(id_value.clone());
        assert_eq!(
            format!("{id:?}"),
            "OutputDeviceId([REDACTED])",
            "persistent id debug is constant"
        );

        let info = OutputDeviceInfo {
            id,
            display_name: name_value.clone(),
            is_default,
        };
        let debug = format!("{info:?}");
        assert!(!debug.contains(&id_marker), "persistent id must never leak");
        assert!(
            !debug.contains(&id_value),
            "full persistent id must never leak"
        );
        assert!(
            !debug.contains(&name_marker),
            "display name must never leak"
        );
        assert!(
            !debug.contains(&name_value),
            "full display name must never leak"
        );
        assert!(
            debug.contains("display_name_present"),
            "debug publishes the display name presence"
        );
        assert!(
            debug.contains("is_default"),
            "debug publishes the default flag"
        );
    }

    // An empty display name publishes `false` instead of the empty value.
    let info = OutputDeviceInfo {
        id: OutputDeviceId::new("opaque"),
        display_name: String::new(),
        is_default: false,
    };
    let debug = format!("{info:?}");
    assert!(debug.contains("display_name_present: false"));
    assert!(!debug.contains("opaque"));
}

/// Free-form backend messages must never appear in `Debug` output but must
/// remain readable through the user-facing `Display` surface.
#[test]
fn randomized_platform_errors_redact_free_form_messages() {
    const STEPS: usize = 5_000;
    let mut rng = XorShift64(0x504C_0000_2026_0012);

    for _ in 0..STEPS {
        let (message, marker) = random_value(&mut rng);
        let factory = RenderSourceFactoryError::Build(message.clone());
        let output = OutputError::Backend(message.clone());

        let factory_debug = format!("{factory:?}");
        let output_debug = format!("{output:?}");
        assert!(
            factory_debug.contains("message_len"),
            "Build debug keeps the message length"
        );
        assert!(
            output_debug.contains("message_len"),
            "Backend debug keeps the message length"
        );
        assert!(
            !factory_debug.contains(&marker),
            "Build debug must not leak the message"
        );
        assert!(
            !output_debug.contains(&marker),
            "Backend debug must not leak the message"
        );
        assert!(
            !factory_debug.contains(&message),
            "Build debug must not leak the full message"
        );
        assert!(
            !output_debug.contains(&message),
            "Backend debug must not leak the full message"
        );

        assert!(
            format!("{factory}").contains(&message),
            "Build display keeps the actionable message"
        );
        assert!(
            format!("{output}").contains(&message),
            "Backend display keeps the actionable message"
        );
    }
}
