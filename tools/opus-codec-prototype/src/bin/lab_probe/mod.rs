//! Opt-in, bounded probe detection outside the output callback.
use sonimesh_latency_measurement::{OffsetConfig, OffsetError, estimate_offset_ms, test_signal};

const RATE: usize = 48_000;
const MAX_SAMPLES: usize = RATE * 30;
const SEGMENT_SAMPLES: usize = 32_768;
const SEGMENT_STARTS: [usize; 4] = [
    0,
    SEGMENT_SAMPLES,
    SEGMENT_SAMPLES * 2,
    131_071 - SEGMENT_SAMPLES,
];

pub struct ProbeCheck {
    samples: Vec<f32>,
}

impl ProbeCheck {
    pub fn new() -> Self {
        Self {
            samples: Vec::with_capacity(MAX_SAMPLES),
        }
    }

    pub fn observe(&mut self, interleaved: &[f32]) {
        let remaining = MAX_SAMPLES - self.samples.len();
        self.samples.extend(
            interleaved
                .chunks_exact(2)
                .take(remaining)
                .map(|pair| (pair[0] + pair[1]) * 0.5),
        );
    }

    pub fn detect(&self) -> Result<(), OffsetError> {
        let reference = test_signal(48_000, 3, 7);
        let mut config = OffsetConfig::new(48_000);
        config.max_search_ms = 30_000.0;
        let first = estimate_offset_ms(&config, &reference, &self.samples).map(|_| ());
        if first.is_ok() || self.samples.iter().all(|sample| *sample == 0.0) {
            return first;
        }
        for start in SEGMENT_STARTS.into_iter().skip(1) {
            let segment = &reference[start..start + SEGMENT_SAMPLES];
            if estimate_offset_ms(&config, segment, &self.samples).is_ok() {
                return Ok(());
            }
        }
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_probe_after_leading_silence() {
        let mut check = ProbeCheck::new();
        check.observe(&vec![0.0; 960 * 2]);
        let reference = test_signal(48_000, 3, 7);
        let stereo: Vec<_> = reference
            .iter()
            .flat_map(|sample| [*sample * 0.1; 2])
            .collect();
        check.observe(&stereo);
        assert!(check.detect().is_ok());
    }

    #[test]
    fn detects_later_segment_when_probe_start_is_missing() {
        let mut check = ProbeCheck::new();
        check.observe(&vec![0.0; 960 * 2]);
        let reference = test_signal(48_000, 3, 7);
        let stereo: Vec<_> = reference[SEGMENT_SAMPLES..]
            .iter()
            .flat_map(|sample| [*sample * 0.1; 2])
            .collect();
        check.observe(&stereo);
        let mut config = OffsetConfig::new(48_000);
        config.max_search_ms = 30_000.0;
        assert!(estimate_offset_ms(&config, &reference, &check.samples).is_err());
        assert!(check.detect().is_ok());
    }

    #[test]
    fn unrelated_signal_is_not_detected() {
        let mut check = ProbeCheck::new();
        let mut state = 123_u32;
        let stereo: Vec<_> = (0..RATE * 3)
            .flat_map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let sample = if state & 0x8000_0000 == 0 { -0.1 } else { 0.1 };
                [sample; 2]
            })
            .collect();
        check.observe(&stereo);
        assert!(check.detect().is_err());
    }

    #[test]
    fn periodic_tone_is_not_detected() {
        let mut check = ProbeCheck::new();
        let tone: Vec<_> = (0..RATE * 3)
            .flat_map(|index| {
                let sample = if (index / 24) % 2 == 0 { 0.1 } else { -0.1 };
                [sample; 2]
            })
            .collect();
        check.observe(&tone);
        assert!(check.detect().is_err());
    }

    #[test]
    fn silence_is_not_detected_and_storage_is_bounded() {
        let mut check = ProbeCheck::new();
        check.observe(&vec![0.0; (MAX_SAMPLES + 960) * 2]);
        assert_eq!(check.samples.len(), MAX_SAMPLES);
        assert!(check.detect().is_err());
    }
}
