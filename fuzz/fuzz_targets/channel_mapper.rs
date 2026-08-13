#![no_main]

//! Fuzz the bounded interleaved channel mapper.
//!
//! Arbitrary negotiated input/output channel layouts meet arbitrary buffer
//! lengths and finite sample payloads. The mapper must never panic, must
//! report exactly the mapped output samples on success, must keep every
//! output sample finite, and must initialize the whole output to silence on
//! every rejected alignment or capacity step.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{ChannelMapper, MAX_AUDIO_CHANNELS};

const MAX_SCRATCH_SAMPLES: usize = 1_024;

fuzz_target!(|data: &[u8]| {
    if data.len() < 8 {
        return;
    }

    let input_channels = 1 + u16::from_le_bytes([data[0], data[1]]) % MAX_AUDIO_CHANNELS;
    let output_channels = 1 + u16::from_le_bytes([data[2], data[3]]) % MAX_AUDIO_CHANNELS;

    // Out-of-range constructor inputs must stay rejected without panicking.
    let invalid_channels = u16::from_le_bytes([data[4], data[5]]) % (MAX_AUDIO_CHANNELS + 2);
    if invalid_channels == 0 || invalid_channels > MAX_AUDIO_CHANNELS {
        assert!(ChannelMapper::new(invalid_channels, output_channels).is_err());
        assert!(ChannelMapper::new(input_channels, invalid_channels).is_err());
    }

    let mapper = ChannelMapper::new(input_channels, output_channels)
        .expect("bounded channel layouts are valid");
    assert_eq!(mapper.input_channels(), input_channels);
    assert_eq!(mapper.output_channels(), output_channels);

    let input_len = usize::from(data[6]) % MAX_SCRATCH_SAMPLES;
    let output_len = usize::from(data[7]) % MAX_SCRATCH_SAMPLES;

    // Deterministic finite samples in [1.0, 2.0): the mapper copies or
    // averages them, so finite input must yield finite output.
    let mut input = [0.0f32; MAX_SCRATCH_SAMPLES];
    for index in 0..input_len {
        let bits = u32::from_le_bytes([
            data[index % data.len()],
            data[(index + 1) % data.len()],
            data[(index + 2) % data.len()],
            data[(index + 3) % data.len()],
        ]);
        input[index] = f32::from_bits(0x3f80_0000 | (bits & 0x007f_ffff));
    }

    let mut output = [0.0f32; MAX_SCRATCH_SAMPLES];
    let output_slice = &mut output[..output_len];
    match mapper.map(&input[..input_len], output_slice) {
        Ok(produced) => {
            let frames = input_len / usize::from(input_channels);
            assert_eq!(produced, frames * usize::from(output_channels));
            assert!(output_slice.iter().all(|sample| sample.is_finite()));
        }
        // A rejected alignment or capacity step must leave the whole output as
        // initialized silence, never uninitialized memory.
        Err(_) => {
            assert!(output_slice.iter().all(|sample| *sample == 0.0));
        }
    }
});
