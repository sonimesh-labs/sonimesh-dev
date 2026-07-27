//! Deterministic source-endpoint policy tests across protected media state.

use core::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use sonimesh_app::{FreshSessionInstallError, ReceiverPipelineError, ReceiverStreamPipeline};
use sonimesh_audio::{FrameDecoder, InsertOutcome, realtime_pcm_queue};
use sonimesh_diagnostics::StreamCounters;
use sonimesh_network::{SourceEndpointAdmissionError, SourceEndpointPin};
use sonimesh_network_simulator::{NetworkProfile, SendOutcome, VirtualNetwork};
use sonimesh_protocol::{
    MAX_MEDIA_PLAINTEXT_LEN, MediaDirection, MediaKeyContext, MediaSealer, SessionSecret,
};

const SESSION_ID: u64 = 0x0102_0304_0506_0708;
const STREAM_ID: u32 = 9;
const INITIAL_EPOCH: u16 = 1;

#[derive(Clone, Debug)]
struct VirtualDatagram {
    source: SocketAddr,
    payload: Vec<u8>,
}

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

type Pipeline = ReceiverStreamPipeline<ByteDecoder, 4, 8, 4>;
type AdmissionError = SourceEndpointAdmissionError<ReceiverPipelineError<Infallible>>;

struct SourceBoundPipeline {
    source_pin: SourceEndpointPin,
    pipeline: Pipeline,
    plaintext: [u8; MAX_MEDIA_PLAINTEXT_LEN],
}

impl SourceBoundPipeline {
    fn new(secret: &SessionSecret, context: MediaKeyContext) -> Self {
        let (producer, _consumer) = realtime_pcm_queue::<4>(4).unwrap();
        Self {
            source_pin: SourceEndpointPin::new(),
            pipeline: Pipeline::new(
                secret,
                context,
                ByteDecoder,
                producer,
                Arc::new(StreamCounters::default()),
                0,
            )
            .unwrap(),
            plaintext: [0; MAX_MEDIA_PLAINTEXT_LEN],
        }
    }

    fn receive(&mut self, datagram: &VirtualDatagram) -> Result<InsertOutcome, AdmissionError> {
        let Self {
            source_pin,
            pipeline,
            plaintext,
        } = self;
        source_pin.process(datagram.source, || {
            pipeline.receive(&datagram.payload, plaintext)
        })
    }

    fn install_fresh_session(
        &mut self,
        secret: &SessionSecret,
        context: MediaKeyContext,
    ) -> Result<(), FreshSessionInstallError> {
        self.pipeline.install_fresh_session(secret, context, 0)?;
        self.source_pin.clear_for_fresh_session();
        Ok(())
    }
}

fn context(session_id: u64, key_epoch: u16) -> MediaKeyContext {
    MediaKeyContext::new(
        session_id,
        STREAM_ID,
        key_epoch,
        MediaDirection::AndroidToDesktop,
    )
    .unwrap()
}

fn endpoint(last_octet: u8, port: u16) -> SocketAddr {
    ([192, 0, 2, last_octet], port).into()
}

fn stable_network() -> VirtualNetwork<VirtualDatagram> {
    let mut profile = NetworkProfile::stable(0xADD2_2026);
    profile.base_delay_ms = 0;
    profile.jitter_ms = 0;
    VirtualNetwork::new(profile).unwrap()
}

fn deliver(
    network: &mut VirtualNetwork<VirtualDatagram>,
    now_ms: u64,
    datagram: VirtualDatagram,
) -> VirtualDatagram {
    let payload_bytes = u64::try_from(datagram.payload.len()).unwrap();
    assert_eq!(
        network.send(now_ms, datagram, payload_bytes),
        SendOutcome::Scheduled { copies: 1 }
    );
    let mut ready = network.drain_ready(now_ms);
    assert_eq!(ready.len(), 1);
    ready.remove(0).payload
}

fn protected_datagram(
    source: SocketAddr,
    sealer: &mut MediaSealer,
    sample_timestamp: u64,
    value: u8,
) -> VirtualDatagram {
    VirtualDatagram {
        source,
        payload: sealer.seal(0, sample_timestamp, &[value]).unwrap().datagram,
    }
}

#[test]
fn unauthenticated_virtual_source_cannot_pin_the_session() {
    let secret = SessionSecret::new([0xA5; 32]);
    let media_context = context(SESSION_ID, INITIAL_EPOCH);
    let mut pipeline = SourceBoundPipeline::new(&secret, media_context);
    let mut network = stable_network();
    let attacker = endpoint(20, 4000);
    let sender = endpoint(10, 4000);

    let invalid = deliver(
        &mut network,
        0,
        VirtualDatagram {
            source: attacker,
            payload: vec![0],
        },
    );
    assert!(matches!(
        pipeline.receive(&invalid),
        Err(SourceEndpointAdmissionError::Processing(_))
    ));
    assert_eq!(pipeline.source_pin.pinned(), None);

    let mut sealer = MediaSealer::new(&secret, media_context).unwrap();
    let valid = deliver(
        &mut network,
        1,
        protected_datagram(sender, &mut sealer, 0, 7),
    );
    assert_eq!(pipeline.receive(&valid).unwrap(), InsertOutcome::Inserted);
    assert_eq!(pipeline.source_pin.pinned(), Some(sender));
}

#[test]
fn changed_virtual_source_cannot_consume_replay_state() {
    let secret = SessionSecret::new([0xA5; 32]);
    let media_context = context(SESSION_ID, INITIAL_EPOCH);
    let mut sealer = MediaSealer::new(&secret, media_context).unwrap();
    let sender = endpoint(10, 4000);
    let changed = endpoint(10, 4001);
    let first = protected_datagram(sender, &mut sealer, 0, 7);
    let second_payload = protected_datagram(changed, &mut sealer, 960, 8).payload;
    let mut pipeline = SourceBoundPipeline::new(&secret, media_context);
    let mut network = stable_network();

    let first = deliver(&mut network, 0, first);
    assert_eq!(pipeline.receive(&first).unwrap(), InsertOutcome::Inserted);

    let changed_copy = deliver(
        &mut network,
        1,
        VirtualDatagram {
            source: changed,
            payload: second_payload.clone(),
        },
    );
    assert!(matches!(
        pipeline.receive(&changed_copy),
        Err(SourceEndpointAdmissionError::SourceEndpointChanged)
    ));

    let original_copy = deliver(
        &mut network,
        2,
        VirtualDatagram {
            source: sender,
            payload: second_payload,
        },
    );
    assert_eq!(
        pipeline.receive(&original_copy).unwrap(),
        InsertOutcome::Inserted
    );
    assert_eq!(pipeline.source_pin.pinned(), Some(sender));
}

#[test]
fn epoch_rotation_preserves_pin_while_fresh_session_rebinds_transactionally() {
    let initial_secret = SessionSecret::new([0xA5; 32]);
    let initial_context = context(SESSION_ID, INITIAL_EPOCH);
    let sender = endpoint(10, 4000);
    let changed = endpoint(20, 4000);
    let mut initial_sealer = MediaSealer::new(&initial_secret, initial_context).unwrap();
    let mut pipeline = SourceBoundPipeline::new(&initial_secret, initial_context);
    let mut network = stable_network();

    let first = deliver(
        &mut network,
        0,
        protected_datagram(sender, &mut initial_sealer, 0, 7),
    );
    assert_eq!(pipeline.receive(&first).unwrap(), InsertOutcome::Inserted);

    let rotated_context = context(SESSION_ID, INITIAL_EPOCH + 1);
    pipeline
        .pipeline
        .rotate(&initial_secret, rotated_context, 0)
        .unwrap();
    let mut rotated_sealer = MediaSealer::new(&initial_secret, rotated_context).unwrap();
    let rotated_payload = protected_datagram(changed, &mut rotated_sealer, 0, 8).payload;
    let changed_rotation = deliver(
        &mut network,
        1,
        VirtualDatagram {
            source: changed,
            payload: rotated_payload.clone(),
        },
    );
    assert!(matches!(
        pipeline.receive(&changed_rotation),
        Err(SourceEndpointAdmissionError::SourceEndpointChanged)
    ));
    let original_rotation = deliver(
        &mut network,
        2,
        VirtualDatagram {
            source: sender,
            payload: rotated_payload,
        },
    );
    assert_eq!(
        pipeline.receive(&original_rotation).unwrap(),
        InsertOutcome::Inserted
    );

    assert_eq!(
        pipeline.install_fresh_session(&SessionSecret::new([0x5A; 32]), rotated_context),
        Err(FreshSessionInstallError::ReusedSessionId)
    );
    assert_eq!(pipeline.source_pin.pinned(), Some(sender));

    let fresh_secret = SessionSecret::new([0x3C; 32]);
    let fresh_context = context(SESSION_ID + 1, 0);
    pipeline
        .install_fresh_session(&fresh_secret, fresh_context)
        .unwrap();
    assert_eq!(pipeline.source_pin.pinned(), None);

    let mut fresh_sealer = MediaSealer::new(&fresh_secret, fresh_context).unwrap();
    let fresh = deliver(
        &mut network,
        3,
        protected_datagram(changed, &mut fresh_sealer, 0, 9),
    );
    assert_eq!(pipeline.receive(&fresh).unwrap(), InsertOutcome::Inserted);
    assert_eq!(pipeline.source_pin.pinned(), Some(changed));
}
