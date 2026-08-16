//! Integration tests for the media-session allocator feeding the receiver pipeline.
//!
//! These tests prove the acknowledged reconnection gap from
//! docs/mvp-baseline.md: an authenticated control-plane reconnection supplies
//! a fresh media session (new media-session identity and key context) through
//! `MediaSessionAllocator`, and `ReceiverStreamPipeline::install_fresh_session`
//! replaces the ingress and replay state so packets sealed under the retired
//! session are rejected while the fresh session's packets are admitted.

use core::convert::Infallible;
use std::sync::Arc;

use sonimesh_app::{
    MAX_RETAINED_MEDIA_IDENTITIES, MediaSessionAllocator, MediaSessionAllocatorError,
    MediaStreamIdentity, ReceiverStreamPipeline,
};
use sonimesh_audio::{DecodeStep, FrameDecoder, InsertOutcome, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
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

const DIRECTION: MediaDirection = MediaDirection::AndroidToDesktop;

const fn identity(session_id: u64, stream_id: u32, key_epoch: u16) -> MediaStreamIdentity {
    MediaStreamIdentity {
        session_id,
        stream_id,
        key_epoch,
    }
}

#[test]
fn allocator_supplies_a_fresh_reconnection_session_to_the_pipeline() {
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0x3C; 32]));

    // First authenticated session installs and admits its own packets.
    let (mut old_sealer, mut pipeline) = {
        let first_context = allocator
            .supply(identity(7, 1, 0), DIRECTION)
            .expect("first identity is fresh");
        let root = allocator.root();
        let sealer = MediaSealer::new(root, first_context).expect("first sealer");
        let (producer, _consumer) = realtime_pcm_queue::<4>(4).expect("pcm queue");
        let pipeline = ReceiverStreamPipeline::<ByteDecoder, 4, 8, 4>::new(
            root,
            first_context,
            ByteDecoder,
            producer,
            Arc::new(StreamCounters::default()),
            0,
        )
        .expect("pipeline with first session");
        (sealer, pipeline)
    };
    let old_packet = old_sealer.seal(0, 0, &[1]).expect("old packet");
    let mut plaintext = [0_u8; 8];
    assert_eq!(
        pipeline
            .receive(&old_packet.datagram, &mut plaintext)
            .expect("old session packet admits"),
        InsertOutcome::Inserted
    );

    // An authenticated reconnection supplies a fresh session (new session id).
    let fresh_context = allocator
        .supply(identity(8, 1, 0), DIRECTION)
        .expect("new session id is fresh");
    pipeline
        .install_fresh_session(allocator.root(), fresh_context, 0)
        .expect("fresh session installs");
    assert_eq!(pipeline.context(), fresh_context);

    // The retired session's packets are rejected after the fresh install.
    assert!(
        pipeline
            .receive(&old_packet.datagram, &mut plaintext)
            .is_err(),
        "old session packet must be rejected after fresh install"
    );

    // The fresh session's packets are admitted and decode.
    let mut fresh_sealer = MediaSealer::new(allocator.root(), fresh_context).expect("fresh sealer");
    let fresh_packet = fresh_sealer.seal(0, 0, &[2]).expect("fresh packet");
    assert_eq!(
        pipeline
            .receive(&fresh_packet.datagram, &mut plaintext)
            .expect("fresh session packet admits"),
        InsertOutcome::Inserted
    );
    assert!(matches!(
        pipeline.decode_next().expect("decode fresh packet"),
        DecodeStep::Decoded { sequence: 0, .. }
    ));
}

#[test]
fn allocator_rejects_a_replayed_identity_after_reconnection() {
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0x3C; 32]));

    allocator
        .supply(identity(7, 1, 0), DIRECTION)
        .expect("first identity is fresh");
    allocator
        .supply(identity(8, 1, 0), DIRECTION)
        .expect("fresh reconnection session");

    // A stale or replayed reconnection that reuses the retired identity must
    // fail closed instead of re-installing the old session.
    let error = allocator
        .supply(identity(7, 1, 0), DIRECTION)
        .expect_err("reused identity must be rejected");
    assert_eq!(error, MediaSessionAllocatorError::IdentityReused);
    assert_eq!(allocator.issued_count(), 2);
}

#[test]
fn a_fresh_pairing_root_resets_the_no_reuse_record() {
    // A fresh pairing root replaces the allocator entirely (ADR 0017), so the
    // new instance may issue an identity that the old pairing used, and the
    // two roots derive distinct AEAD keys for the same context tuple.
    let old_packet = {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0x3C; 32]));
        let context = allocator
            .supply(identity(7, 1, 0), DIRECTION)
            .expect("first pairing identity");
        let mut sealer = MediaSealer::new(allocator.root(), context).unwrap();
        sealer.seal(0, 0, &[1]).unwrap()
    };
    let fresh_packet = {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0x5A; 32]));
        let context = allocator
            .supply(identity(7, 1, 0), DIRECTION)
            .expect("fresh root may reuse the identity with new key material");
        let mut sealer = MediaSealer::new(allocator.root(), context).unwrap();
        sealer.seal(0, 0, &[2]).unwrap()
    };

    assert_ne!(old_packet.datagram, fresh_packet.datagram);
}

#[test]
fn the_no_reuse_record_capacity_is_pinned_and_exposed() {
    assert_eq!(MAX_RETAINED_MEDIA_IDENTITIES, 16);
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0x3C; 32]));

    for index in 0..MAX_RETAINED_MEDIA_IDENTITIES {
        allocator
            .supply(identity(100 + index as u64, 1, 0), DIRECTION)
            .unwrap_or_else(|error| panic!("identity {index} must be fresh: {error}"));
    }
    assert_eq!(allocator.issued_count(), MAX_RETAINED_MEDIA_IDENTITIES);
}

#[test]
fn issued_context_matches_the_protocol_key_context() {
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0x3C; 32]));

    let context = allocator.supply(identity(7, 3, 5), DIRECTION).unwrap();

    let expected = MediaKeyContext::new(7, 3, 5, DIRECTION).unwrap();
    assert_eq!(context, expected);
}
