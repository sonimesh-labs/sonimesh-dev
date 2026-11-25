//! Opt-in, bounded probe detection outside the output callback.
use sonimesh_latency_measurement::{
    OffsetConfig, OffsetError, OffsetEstimate, estimate_offset_ms, test_signal,
};

const RATE: usize = 48_000;
const MAX_SAMPLES: usize = RATE * 30;

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

    pub fn detect(&self) -> Result<OffsetEstimate, OffsetError> {
        let reference = test_signal(48_000, 3, 7);
        let mut config = OffsetConfig::new(48_000);
        config.max_search_ms = 30_000.0;
        estimate_offset_ms(&config, &reference, &self.samples)
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
    fn silence_is_not_detected_and_storage_is_bounded() {
        let mut check = ProbeCheck::new();
        check.observe(&vec![0.0; (MAX_SAMPLES + 960) * 2]);
        assert_eq!(check.samples.len(), MAX_SAMPLES);
        assert!(check.detect().is_err());
    }
}
