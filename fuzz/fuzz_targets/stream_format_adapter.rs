#![no_main]

//! Fuzz the per-stream source-to-output format adapter.
//!
//! `StreamFormatAdapter` bridges a decoded source queue to a Phase 2 mixer
//! slot queue (ADR 0015): it drains decoded source blocks, applies nominal
//! resampling and channel mapping, and pushes bounded output-format blocks.
//! Arbitrary decoded source bytes, drift corrections, and epoch resets must
//! never panic the pump, exceed the fixed source or output queue bound, or
//! leave non-finite samples in a consumer-visible output block.

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{PumpOutcome, SourcePcmFormat, StreamFormatAdapter};
use sonimesh_audio::{RealtimePcmPopOutcome, RealtimePcmPushOutcome, realtime_pcm_queue};

const SOURCE_SAMPLES_PER_BLOCK: usize = 8;
const OUTPUT_SAMPLES_PER_BLOCK: usize = 8;
const SOURCE_QUEUE_CAPACITY: usize = 4;
const OUTPUT_QUEUE_CAPACITY: usize = 4;
const MAX_OUTPUT_FRAMES: usize = OUTPUT_SAMPLES_PER_BLOCK / 2;

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }

    let (mut source_producer, source_consumer) =
        realtime_pcm_queue::<SOURCE_SAMPLES_PER_BLOCK>(SOURCE_QUEUE_CAPACITY)
            .expect("fixed source queue is valid");
    let (output_producer, mut output_consumer) =
        realtime_pcm_queue::<OUTPUT_SAMPLES_PER_BLOCK>(OUTPUT_QUEUE_CAPACITY)
            .expect("fixed output queue is valid");
    let source_format = SourcePcmFormat::new(48_000, 1).expect("mono source format");
    let output_format = SourcePcmFormat::new(48_000, 2).expect("stereo output format");
    let mut adapter =
        StreamFormatAdapter::<SOURCE_SAMPLES_PER_BLOCK, OUTPUT_SAMPLES_PER_BLOCK>::new(
            source_consumer,
            output_producer,
            source_format,
            output_format,
        )
        .expect("fixed adapter configuration is valid");

    // Push arbitrary decoded source blocks: every sample is finite and the
    // bounded queue must never grow past its capacity.
    let block_count = data.len().min(SOURCE_QUEUE_CAPACITY + 2);
    for index in 0..block_count {
        let byte = data[index % data.len()];
        let mut block = [0.0f32; SOURCE_SAMPLES_PER_BLOCK];
        for sample in &mut block {
            *sample =
                f32::from(byte.wrapping_add(u8::try_from(index).expect("small index"))) / 128.0;
        }
        let _ = source_producer.push(&block);
    }

    // Drive pump steps with adversarial drift corrections and periodic epoch
    // resets. Every outcome must stay inside the fixed bounds or fail closed.
    for step in 0..16 {
        let ppm = i16::from_le_bytes([data[step % data.len()], data[(step + 1) % data.len()]]);
        let _ = adapter.set_rate_adjustment_ppm(i32::from(ppm));

        match adapter.pump() {
            Ok(PumpOutcome::Pushed { frames, queue }) => {
                assert!(frames <= MAX_OUTPUT_FRAMES);
                assert!(matches!(
                    queue,
                    RealtimePcmPushOutcome::Inserted | RealtimePcmPushOutcome::ReplacedOldest
                ));
            }
            Ok(PumpOutcome::Underflow) => {}
            // A conversion or queue rejection must leave the adapter usable.
            Err(_) => {}
        }

        if step % 4 == 0 {
            adapter.begin_epoch();
        }
    }

    assert!(adapter.queued_source_blocks() <= SOURCE_QUEUE_CAPACITY);
    assert!(adapter.queued_output_blocks() <= OUTPUT_QUEUE_CAPACITY);

    let mut block = [f32::NAN; OUTPUT_SAMPLES_PER_BLOCK];
    match output_consumer.pop_into(&mut block) {
        RealtimePcmPopOutcome::Filled { samples, .. } => {
            assert!(samples <= OUTPUT_SAMPLES_PER_BLOCK);
            assert!(block[..samples].iter().all(|sample| sample.is_finite()));
        }
        RealtimePcmPopOutcome::Underflow { .. } => {}
    }
});
