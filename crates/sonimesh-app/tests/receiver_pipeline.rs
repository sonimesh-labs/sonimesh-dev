//! Cross-domain receiver pipeline tests with real media AEAD packets.

use core::convert::Infallible;

use sonimesh_app::ReceiverStreamPipeline;
use sonimesh_audio::{
    DecodeStep, FrameDecoder, InsertOutcome, RealtimePcmPopOutcome, realtime_pcm_queue,
};
use sonimesh_network::IngressError;
use sonimesh_protocol::{MediaDirection, MediaKeyContext, MediaSealer, SessionSecret};

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

fn context(epoch: u16) -> MediaKeyContext {
    MediaKeyContext::new(
        0x0102_0304_0506_0708,
        9,
        epoch,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

#[test]
fn receives_authenticated_media_then_decodes_into_realtime_pcm() {
    let secret = SessionSecret::new([0xA5; 32]);
    let context = context(2);
    let mut sealer = MediaSealer::new(&secret, context).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        context,
        ByteDecoder,
        producer,
        0,
    )
    .unwrap();
    let packet = sealer.seal(0, 0, &[7]).unwrap();
    let mut plaintext = [0_u8; 8];

    assert_eq!(
        pipeline.receive(&packet.datagram, &mut plaintext).unwrap(),
        InsertOutcome::Inserted
    );
    assert!(matches!(
        pipeline.decode_next().unwrap(),
        DecodeStep::Decoded { sequence: 0, .. }
    ));
    let mut output = [f32::NAN; 4];
    assert_eq!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            samples: 1,
            sequence: 0,
            discontinuity: false,
            discarded_stale_blocks: 0,
        }
    );
    assert!((output[0] - 7.0).abs() < f32::EPSILON);
}

#[test]
fn rotation_rejects_old_packets_and_retires_old_pcm() {
    let secret = SessionSecret::new([0xA5; 32]);
    let initial = context(2);
    let mut old_sealer = MediaSealer::new(&secret, initial).unwrap();
    let (producer, mut consumer) = realtime_pcm_queue::<4>(4).unwrap();
    let mut pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
        &secret,
        initial,
        ByteDecoder,
        producer,
        0,
    )
    .unwrap();
    let old_packet = old_sealer.seal(0, 0, &[1]).unwrap();
    let mut plaintext = [0_u8; 8];

    pipeline
        .receive(&old_packet.datagram, &mut plaintext)
        .unwrap();
    pipeline.decode_next().unwrap();
    let next = context(3);
    pipeline.rotate(&secret, next, 0).unwrap();

    plaintext.fill(0xFF);
    assert!(matches!(
        pipeline.receive(&old_packet.datagram, &mut plaintext),
        Err(sonimesh_app::ReceiverPipelineError::Ingress(
            IngressError::UnknownStream
        ))
    ));
    assert!(plaintext.iter().all(|byte| *byte == 0));

    let mut new_sealer = MediaSealer::new(&secret, next).unwrap();
    let new_packet = new_sealer.seal(0, 0, &[2]).unwrap();
    pipeline
        .receive(&new_packet.datagram, &mut plaintext)
        .unwrap();
    pipeline.decode_next().unwrap();

    let mut output = [f32::NAN; 4];
    assert!(matches!(
        consumer.pop_into(&mut output),
        RealtimePcmPopOutcome::Filled {
            sequence: 0,
            discarded_stale_blocks: 1,
            ..
        }
    ));
    assert!((output[0] - 2.0).abs() < f32::EPSILON);
}
