#![no_main]

//! Fuzz the bounded polyphase nominal-rate converter.
//!
//! The windowed-sinc resampler converts between a sender-declared input rate
//! and the selected output rate; both arrive from a remote peer or a device
//! descriptor rather than from local trusted code. Arbitrary rate, channel,
//! drift, and buffer-length inputs must never panic the stateful convolution
//! loop, exceed the returned consumed/produced bounds, or leave uninitialized
//! output on a rejected step.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{
    MAX_AUDIO_CHANNELS, MAX_RESAMPLE_RATE_HZ, MIN_RESAMPLE_RATE_HZ, PolyphaseResampler,
};

const MAX_SCRATCH_SAMPLES: usize = 4_096;

fuzz_target!(|data: &[u8]| {
    if data.len() < 12 {
        return;
    }

    let input_rate = MIN_RESAMPLE_RATE_HZ
        + u32::from_le_bytes([data[0], data[1], data[2], data[3]])
            % (MAX_RESAMPLE_RATE_HZ - MIN_RESAMPLE_RATE_HZ + 1);
    let output_rate = MIN_RESAMPLE_RATE_HZ
        + u32::from_le_bytes([data[4], data[5], data[6], data[7]])
            % (MAX_RESAMPLE_RATE_HZ - MIN_RESAMPLE_RATE_HZ + 1);
    let channels = 1 + u16::from_le_bytes([data[8], data[9]]) % MAX_AUDIO_CHANNELS;
    let adjustment_ppm = i16::from_le_bytes([data[10], data[11]]) as i32;

    let mut resampler = match PolyphaseResampler::new(input_rate, output_rate, channels) {
        Ok(resampler) => resampler,
        // The bounded constructor inputs above stay valid; keep the rejection
        // path compiled and exercised for future bound changes.
        Err(_) => return,
    };
    // Both the accepted and the out-of-range drift paths must stay safe.
    let _ = resampler.set_rate_adjustment_ppm(adjustment_ppm);

    // Deterministic finite scratch samples in [1.0, 2.0): the converter must
    // never panic on payload values and must keep finite output while it
    // convolves them.
    let mut scratch = [0.0f32; MAX_SCRATCH_SAMPLES];
    for index in 0..scratch.len() {
        let first = u32::from(data[index % data.len()]);
        let second = u32::from(data[(index + 3) % data.len()]);
        scratch[index] = f32::from_bits(0x3f80_0000 | (first << 15) | (second << 7));
    }

    for step in 0..4 {
        let input_samples = usize::from(data[(step * 2) % data.len()]) % MAX_SCRATCH_SAMPLES;
        let output_samples = usize::from(data[(step * 2 + 1) % data.len()]) % MAX_SCRATCH_SAMPLES;
        let output_frames = usize::from(data[(step + 8) % data.len()]) % 256;

        let _ = resampler.input_frames_for_output(output_frames);

        let input = &scratch[..input_samples];
        let mut output = [0.0f32; MAX_SCRATCH_SAMPLES];
        let output_slice = &mut output[..output_samples];

        match resampler.process(input, output_slice) {
            Ok(result) => {
                let input_frames = input.len() / usize::from(channels);
                let produced_frames = output_slice.len() / usize::from(channels);
                assert!(result.consumed_input_frames <= input_frames);
                assert!(result.produced_output_frames <= produced_frames);
                assert!(output_slice.iter().all(|sample| sample.is_finite()));
            }
            // A frame-alignment rejection must leave the whole output as
            // initialized silence, never uninitialized memory.
            Err(_) => {
                assert!(output_slice.iter().all(|sample| *sample == 0.0));
            }
        }
        resampler.reset();
    }
});
