//! Control-domain composition tests for stream-local drift actions.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{ReceiverStreamPipeline, SampleRateMappedRenderSource, StreamDriftController};
use sonimesh_audio::{
    DecodeStep, DriftConfig, DriftDecision, DriftResyncDirection, FrameDecoder,
    RealtimePcmPopOutcome, realtime_pcm_queue,
};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_platform::{MAX_CALLBACK_FRAMES, RenderSource};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

const CONFIG: DriftConfig = DriftConfig {
    target_blocks: 2,
    low_resync_blocks: 0,
    high_resync_blocks: 4,
    proportional_ppm_per_block: 100,
    integral_ppm_per_block_tick: 10,
    max_adjustment_ppm: 500,
    max_step_ppm: 500,
    max_integral_block_ticks: 16,
};

struct ByteDecoder;

impl FrameDecoder for ByteDecoder {
    type Error = Infallible;

    fn decode(&mut self, encoded: &[u8], output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = f32::from(encoded[0]);
        Ok(1)
    }

    fn decode_plc(&mut self, output: &mut [f32]) -> Result<usize, Self::Error> {
        output[0] = 0.0;
        Ok(1)
    }
}

struct SilenceSource;

impl RenderSource for SilenceSource {
    fn render(&mut self, output: &mut [f32]) {
        output.fill(0.0);
    }
}

fn context() -> MediaKeyContext {
    MediaKeyContext::new(
        0x0102_0304_0506_0708,
        9,
        2,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

#[test]
fn control_tick_publishes_ppm_then_retires_the_affected_stream() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context();
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let counters = Arc::new(StreamCounters::default());
    let (_source, adjustment) = SampleRateMappedRenderSource::new_with_drift_control_and_metrics(
        Box::new(SilenceSource),
        48_000,
        48_000,
        1,
        MAX_CALLBACK_FRAMES,
        Some(Arc::clone(&counters)),
    )
    .unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        context,
        ByteDecoder,
        producer,
        Arc::clone(&counters),
        0,
    )
    .unwrap();
    let mut controller = StreamDriftController::new(CONFIG, adjustment.clone()).unwrap();
    let mut plaintext = [0_u8; 8];

    for sequence in 0_u8..3 {
        let packet = sealer
            .seal(0, u64::from(sequence) * 960, &[sequence])
            .unwrap();
        pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
        assert!(matches!(
            pipeline.decode_next().unwrap(),
            DecodeStep::Decoded { sequence: actual, .. } if actual == u32::from(sequence)
        ));
    }
    assert_eq!(pipeline.queued_pcm_blocks(), 3);

    assert_eq!(
        controller.update(&mut pipeline),
        Ok(DriftDecision::Adjust { ppm: 110 })
    );
    assert_eq!(controller.adjustment_ppm(), 110);
    assert_eq!(adjustment.current_ppm(), 110);
    let adjusted = counters.snapshot();
    assert_eq!(adjusted.drift_adjustment_ppm, 110);
    assert_eq!(adjusted.queued_pcm_blocks, 3);
    assert_eq!(adjusted.queued_pcm_high_water_blocks, 3);

    let packet = sealer.seal(0, 2_880, &[3]).unwrap();
    pipeline.receive(&packet.datagram, &mut plaintext).unwrap();
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 3, .. }
    ));
    assert_eq!(pipeline.queued_pcm_blocks(), 4);

    assert_eq!(
        controller.update(&mut pipeline),
        Ok(DriftDecision::HardResync {
            direction: DriftResyncDirection::High,
        })
    );
    assert_eq!(controller.adjustment_ppm(), 0);
    assert_eq!(adjustment.current_ppm(), 0);
    let snapshot = counters.snapshot();
    assert_eq!(snapshot.drift_adjustment_ppm, 0);
    assert_eq!(snapshot.drift_hard_resyncs, 1);
    assert_eq!(snapshot.stream_resets, 1);
    assert_eq!(snapshot.queued_pcm_blocks, 0);
    assert_eq!(snapshot.queued_pcm_high_water_blocks, 0);
    assert_eq!(pipeline.queued_pcm_blocks(), 0);
    let mut output = [f32::NAN; 4];
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Underflow {
            discarded_stale_blocks: 4,
        }
    );
}
