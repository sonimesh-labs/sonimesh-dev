//! Cross-crate deterministic media-pipeline regression test.

use std::fmt;

use sonimesh_audio::{
    EncodedFrame, InsertOutcome, JitterBuffer, PlayoutOutcome, RealtimePcmPopOutcome,
    RealtimePcmPushOutcome, realtime_pcm_queue,
};
use sonimesh_network::{Authenticator, IngressError, MediaIngress, StreamIdentity};
use sonimesh_network_simulator::{NetworkProfile, VirtualNetwork};
use sonimesh_protocol::MediaHeader;

const FRAMES: u32 = 200;
const FRAME_MS: u64 = 20;
const TARGET_MS: u64 = 60;
const SESSION_ID: u64 = 7;
const STREAM_ID: u32 = 9;
const KEY_EPOCH: u16 = 1;

#[derive(Debug, Eq, PartialEq)]
struct LabAuthenticationError;

impl fmt::Display for LabAuthenticationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("lab marker authentication failed")
    }
}

impl std::error::Error for LabAuthenticationError {}

struct LabAuthenticator;

impl Authenticator for LabAuthenticator {
    type Error = LabAuthenticationError;

    fn authenticate(
        &mut self,
        _authenticated_header: &[u8],
        ciphertext: &[u8],
        plaintext: &mut [u8],
    ) -> Result<usize, Self::Error> {
        let Some((&0xA5, payload)) = ciphertext.split_first() else {
            return Err(LabAuthenticationError);
        };
        if payload.len() > plaintext.len() {
            return Err(LabAuthenticationError);
        }
        plaintext[..payload.len()].copy_from_slice(payload);
        Ok(payload.len())
    }
}

fn media_datagram(sequence: u32) -> Vec<u8> {
    let mut ciphertext = Vec::with_capacity(5);
    ciphertext.push(0xA5);
    ciphertext.extend_from_slice(&sequence.to_be_bytes());
    let header = MediaHeader {
        flags: 0,
        session_id: SESSION_ID,
        stream_id: STREAM_ID,
        sequence,
        sample_timestamp: u64::from(sequence) * 960,
        key_epoch: KEY_EPOCH,
    };
    let mut datagram = header.encode(ciphertext.len()).unwrap().to_vec();
    datagram.extend_from_slice(&ciphertext);
    datagram
}

#[test]
fn authenticated_pipeline_stays_bounded_under_impairment() {
    let mut profile = NetworkProfile::stable(0xC0DE_2026);
    profile.loss_percent = 3;
    profile.duplicate_percent = 20;
    profile.reorder_percent = 20;
    profile.jitter_ms = 10;
    profile.reorder_delay_ms = 25;
    let mut network = VirtualNetwork::new(profile).unwrap();
    let mut ingress = MediaIngress::new(
        StreamIdentity {
            session_id: SESSION_ID,
            stream_id: STREAM_ID,
            key_epoch: KEY_EPOCH,
        },
        LabAuthenticator,
    );
    let mut jitter = JitterBuffer::<16, 8>::with_start_sequence(0);
    let (mut pcm_producer, mut pcm_consumer) = realtime_pcm_queue::<1>(4).unwrap();
    let mut plaintext = [0_u8; 8];
    let mut next_send = 0_u32;
    let mut next_playout = 0_u32;
    let mut replayed = 0_u32;
    let mut rejected = 0_u32;
    let mut plc_frames = 0_u32;
    let mut rendered = 0_u32;
    let mut jitter_resets = 0_u32;
    let end_ms = TARGET_MS + u64::from(FRAMES - 1) * FRAME_MS + 100;

    for now_ms in 0..=end_ms {
        if next_send < FRAMES && now_ms == u64::from(next_send) * FRAME_MS {
            network.send(now_ms, media_datagram(next_send));
            next_send += 1;
        }

        for delivery in network.drain_ready(now_ms) {
            match ingress.receive(&delivery.payload, &mut plaintext) {
                Ok(frame) => {
                    let encoded = EncodedFrame::new(
                        frame.header.sequence,
                        frame.header.sample_timestamp,
                        frame.opus_payload,
                    )
                    .unwrap();
                    match jitter.insert(encoded) {
                        InsertOutcome::Inserted => {}
                        InsertOutcome::Resynchronized { .. } => jitter_resets += 1,
                        InsertOutcome::Duplicate | InsertOutcome::Late => rejected += 1,
                        InsertOutcome::Disabled => panic!("test jitter buffer must be enabled"),
                    }
                }
                Err(IngressError::Replay(_)) => replayed += 1,
                Err(_) => rejected += 1,
            }
        }

        if next_playout < FRAMES && now_ms == TARGET_MS + u64::from(next_playout) * FRAME_MS {
            let sample = match jitter.take_next() {
                PlayoutOutcome::Frame(frame) => {
                    assert_eq!(frame.payload().len(), 4);
                    1.0
                }
                PlayoutOutcome::Missing { sequence } => {
                    assert_eq!(sequence, next_playout);
                    plc_frames += 1;
                    0.0
                }
                PlayoutOutcome::NotReady => panic!("negotiated playout must produce a decision"),
            };
            assert_eq!(
                pcm_producer.push(&[sample]),
                RealtimePcmPushOutcome::Inserted
            );
            let mut output = [f32::NAN];
            assert_eq!(
                pcm_consumer.pop_into(&mut output),
                RealtimePcmPopOutcome::Filled {
                    samples: 1,
                    sequence: u64::from(next_playout),
                    discontinuity: false,
                }
            );
            assert!(output[0].is_finite());
            rendered += 1;
            next_playout += 1;
        }
    }

    assert_eq!(next_send, FRAMES);
    assert_eq!(rendered, FRAMES);
    assert!(plc_frames > 0);
    assert!(replayed > 0);
    assert_eq!(rejected, 0);
    assert_eq!(jitter_resets, 0);
    assert!(network.stats().random_loss_packets > 0);
    assert!(network.queued_copies() <= profile.max_in_flight_packets);
}
