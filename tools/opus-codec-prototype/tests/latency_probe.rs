//! Software-only check of the latency probe through the native Opus codec.

use sonimesh_latency_measurement::{OffsetConfig, estimate_offset_ms, test_signal};

#[test]
fn mls_probe_remains_correlatable_after_opus_round_trip() {
    let reference = test_signal(48_000, 3, 7);
    for bitrate in [64_000, 96_000, 128_000] {
        check_round_trip(&reference, bitrate);
    }
}

fn check_round_trip(reference: &[f32], bitrate: i32) {
    let mut encoder = opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio)
        .expect("create encoder");
    encoder
        .set_bitrate(opus::Bitrate::Bits(bitrate))
        .expect("set bitrate");
    let lookahead = encoder.get_lookahead().expect("codec lookahead");
    let mut decoder = opus::Decoder::new(48_000, opus::Channels::Stereo).expect("create decoder");
    let lead_in = 1_250;
    let mut recording = vec![0.0; lead_in];
    let mut packet = [0_u8; 1_148];
    let mut pcm = [0_f32; 1_920];
    for frame in reference.chunks_exact(960) {
        let stereo: Vec<_> = frame
            .iter()
            .flat_map(|sample| [sample * 0.0915, sample * 0.0915])
            .collect();
        let bytes = encoder
            .encode_float(&stereo, &mut packet)
            .expect("encode frame");
        let frames = decoder
            .decode_float(&packet[..bytes], &mut pcm, false)
            .expect("decode frame");
        recording.extend(
            pcm[..frames * 2]
                .chunks_exact(2)
                .map(|channel| (channel[0] + channel[1]) * 0.5),
        );
    }
    let mut config = OffsetConfig::new(48_000);
    config.max_search_ms = 100.0;
    let result =
        estimate_offset_ms(&config, reference, &recording).expect("probe must remain detectable");
    let expected = f64::from(lookahead) + 1_250.0;
    assert!(
        (result.lag_samples - expected).abs() < 1.0,
        "unexpected lag at {bitrate} bps: {result:?}, expected {expected}"
    );
}
