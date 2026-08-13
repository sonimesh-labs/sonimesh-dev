//! Deterministic property coverage for the WAVE encoder/decoder pair.
//!
//! The codec must round-trip arbitrary finite samples through every supported
//! format without loss beyond the format's own quantization, and identical
//! input must produce bit-identical output. A small seeded xorshift generator
//! keeps the sweep deterministic without adding a dependency.

use sonimesh_latency_measurement::{SampleFormat, encode_mono, parse_mono};

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
}

/// Uniform pseudo-random sample in `[-1.0, 1.0)`; the `f64` intermediate keeps
/// the grid dense enough to exercise every format's quantization.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn next_sample(rng: &mut XorShift64) -> f32 {
    ((f64::from(rng.next_u32()) / 4_294_967_296.0) * 2.0 - 1.0) as f32
}

/// One quantization step above the format's decode scale, so the round-trip
/// tolerance is dominated by the encoder's own quantization.
const fn tolerance(format: SampleFormat) -> f32 {
    match format {
        SampleFormat::Pcm16 => 2.0 / 32_768.0,
        SampleFormat::Pcm24 => 2.0 / 8_388_608.0,
        SampleFormat::Pcm32 => 2.0 / 2_147_483_648.0,
        SampleFormat::Float32 | SampleFormat::Float64 => 1e-6,
    }
}

const SAMPLE_RATES: &[u32] = &[8_000, 44_100, 48_000, 96_000, 192_000];
const FORMATS: &[SampleFormat] = &[
    SampleFormat::Pcm16,
    SampleFormat::Pcm24,
    SampleFormat::Pcm32,
    SampleFormat::Float32,
    SampleFormat::Float64,
];

/// Random finite samples round-trip through every format and sample rate
/// within the format's own quantization step.
#[test]
fn samples_round_trip_within_quantization_across_formats_and_rates() {
    const STEPS: usize = 300;
    const SAMPLE_COUNT: usize = 256;
    let mut rng = XorShift64(0x0A11_4A55_2026_0042);

    for _ in 0..STEPS {
        let format = FORMATS[rng.next_usize(FORMATS.len())];
        let rate = SAMPLE_RATES[rng.next_usize(SAMPLE_RATES.len())];
        let samples: Vec<f32> = (0..SAMPLE_COUNT).map(|_| next_sample(&mut rng)).collect();

        let bytes = encode_mono(rate, &samples, format);
        let parsed = parse_mono(&bytes).expect("encoded wave parses");
        assert_eq!(parsed.sample_rate_hz, rate, "{format:?} rate {rate}");
        assert_eq!(parsed.samples.len(), SAMPLE_COUNT, "{format:?}");

        let bound = tolerance(format);
        for (actual, expected) in parsed.samples.iter().zip(&samples) {
            assert!(
                (*actual - *expected).abs() <= bound,
                "{format:?} rate {rate}: expected {expected}, observed {actual}"
            );
        }
    }
}

/// Encoding is deterministic, and every `--bits` token round-trips through
/// `SampleFormat::parse`.
#[test]
fn encode_is_deterministic_and_format_tokens_round_trip() {
    let samples = [-1.0_f32, -0.5, 0.0, 0.25, 0.5, 1.0];
    for format in FORMATS {
        let first = encode_mono(48_000, &samples, *format);
        let second = encode_mono(48_000, &samples, *format);
        assert_eq!(first, second, "{format:?} encoding is deterministic");
        assert_eq!(
            SampleFormat::parse(format.name()),
            Some(*format),
            "{format:?} token round-trip"
        );
    }
    assert_eq!(SampleFormat::parse("20"), None);
    assert_eq!(SampleFormat::parse("float32"), None);
}
