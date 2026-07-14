/// Allocation-free linear gain smoothing across callback blocks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GainSmoother {
    current: f32,
    target: f32,
    step: f32,
    remaining_samples: u16,
}

impl GainSmoother {
    /// Starts with a constant gain.
    #[must_use]
    pub const fn new(gain: f32) -> Self {
        Self {
            current: gain,
            target: gain,
            step: 0.0,
            remaining_samples: 0,
        }
    }

    /// Starts a bounded linear ramp. A zero-length ramp applies immediately.
    pub fn set_target(&mut self, target: f32, ramp_samples: u16) {
        self.target = target;
        if ramp_samples == 0 {
            self.current = target;
            self.step = 0.0;
            self.remaining_samples = 0;
            return;
        }
        self.step = (target - self.current) / f32::from(ramp_samples);
        self.remaining_samples = ramp_samples;
    }

    /// Applies the current gain and advances the ramp without allocation.
    pub fn apply(&mut self, samples: &mut [f32]) {
        for sample in samples {
            if self.remaining_samples > 0 {
                self.current += self.step;
                self.remaining_samples -= 1;
                if self.remaining_samples == 0 {
                    self.current = self.target;
                }
            }
            *sample *= self.current;
        }
    }

    /// Current ramp position.
    #[must_use]
    pub const fn current(&self) -> f32 {
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramps_gain_across_blocks() {
        let mut gain = GainSmoother::new(0.0);
        gain.set_target(1.0, 4);
        let mut first = [1.0; 2];
        let mut second = [1.0; 2];

        gain.apply(&mut first);
        gain.apply(&mut second);

        assert!(
            first
                .iter()
                .zip([0.25, 0.5])
                .all(|(left, right)| (left - right).abs() < 1e-6)
        );
        assert!(
            second
                .iter()
                .zip([0.75, 1.0])
                .all(|(left, right)| (left - right).abs() < 1e-6)
        );
        assert!((gain.current() - 1.0).abs() < 1e-6);
    }
}
