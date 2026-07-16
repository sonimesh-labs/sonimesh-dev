//! Wire and semantic boundary tests for length-prefixed control Protobuf frames.

use prost::Message;
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlError, ControlFrameError, Envelope, Hello,
    MAX_CONTROL_CAPABILITIES, MAX_CONTROL_DETAIL_BYTES, MAX_CONTROL_FRAME_BYTES, StartStream,
    StopStream, decode_control_frame, encode_control_frame, envelope,
};

fn hello_envelope() -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 7,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![Capability::FrameDuration20Ms as i32],
            display_name: "Lab sender".to_owned(),
        })),
    }
}

fn raw_frame(envelope: &Envelope) -> Vec<u8> {
    let payload = envelope.encode_to_vec();
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
    frame.extend_from_slice(&payload);
    frame
}

#[test]
fn round_trips_big_endian_framed_hello() {
    let envelope = hello_envelope();
    let frame = encode_control_frame(&envelope).unwrap();

    let declared = u32::from_be_bytes(frame[..4].try_into().unwrap());
    assert_eq!(usize::try_from(declared).unwrap(), frame.len() - 4);
    assert_eq!(decode_control_frame(&frame).unwrap(), envelope);
}

#[test]
fn matches_published_cross_language_control_frame_vector() {
    assert_eq!(
        encode_control_frame(&hello_envelope()).unwrap(),
        [
            0x00, 0x00, 0x00, 0x18, 0x08, 0x01, 0x10, 0x07, 0x52, 0x12, 0x0A, 0x01, 0x01, 0x12,
            0x01, 0x02, 0x1A, 0x0A, 0x4C, 0x61, 0x62, 0x20, 0x73, 0x65, 0x6E, 0x64, 0x65, 0x72,
        ]
    );
}

#[test]
fn rejects_truncated_oversized_and_inconsistent_frames() {
    assert!(matches!(
        decode_control_frame(&[0, 0, 0]),
        Err(ControlFrameError::TruncatedLength)
    ));

    let oversized = u32::try_from(MAX_CONTROL_FRAME_BYTES + 1)
        .unwrap()
        .to_be_bytes();
    assert!(matches!(
        decode_control_frame(&oversized),
        Err(ControlFrameError::FrameTooLarge)
    ));

    let mut inconsistent = encode_control_frame(&hello_envelope()).unwrap();
    inconsistent[3] = inconsistent[3].saturating_add(1);
    assert!(matches!(
        decode_control_frame(&inconsistent),
        Err(ControlFrameError::LengthMismatch)
    ));
}

#[test]
fn rejects_malformed_and_semantically_incomplete_envelopes() {
    assert!(matches!(
        decode_control_frame(&[0, 0, 0, 1, 0xFF]),
        Err(ControlFrameError::Decode(_))
    ));

    let empty = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 1,
        payload: None,
    };
    assert!(matches!(
        decode_control_frame(&raw_frame(&empty)),
        Err(ControlFrameError::MissingPayload)
    ));
}

#[test]
fn rejects_version_and_field_bound_violations() {
    let mut wrong_version = hello_envelope();
    wrong_version.protocol_version += 1;
    assert!(matches!(
        decode_control_frame(&raw_frame(&wrong_version)),
        Err(ControlFrameError::UnsupportedVersion(_))
    ));

    let mut too_many = hello_envelope();
    let Some(envelope::Payload::Hello(hello)) = too_many.payload.as_mut() else {
        panic!("test envelope has hello payload");
    };
    hello.capabilities = vec![0; MAX_CONTROL_CAPABILITIES + 1];
    assert!(matches!(
        encode_control_frame(&too_many),
        Err(ControlFrameError::TooManyCapabilities)
    ));

    let detail = "x".repeat(MAX_CONTROL_DETAIL_BYTES + 1);
    let error = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 9,
        payload: Some(envelope::Payload::Error(ControlError {
            code: 0,
            retryable: false,
            detail,
        })),
    };
    assert!(matches!(
        encode_control_frame(&error),
        Err(ControlFrameError::DetailTooLong)
    ));
}

#[test]
fn preserves_additive_unknown_fields_but_rejects_unknown_commands() {
    let mut payload = hello_envelope().encode_to_vec();
    payload.extend_from_slice(&[0x78, 0x01]);
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
    frame.extend_from_slice(&payload);
    assert!(matches!(
        decode_control_frame(&frame),
        Ok(Envelope {
            payload: Some(envelope::Payload::Hello(_)),
            ..
        })
    ));

    let unknown_only = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 3,
        payload: None,
    };
    let mut unknown_payload = unknown_only.encode_to_vec();
    unknown_payload.extend_from_slice(&[0x7A, 0x00]);
    let mut unknown_frame = Vec::with_capacity(4 + unknown_payload.len());
    unknown_frame.extend_from_slice(&u32::try_from(unknown_payload.len()).unwrap().to_be_bytes());
    unknown_frame.extend_from_slice(&unknown_payload);
    assert!(matches!(
        decode_control_frame(&unknown_frame),
        Err(ControlFrameError::MissingPayload)
    ));
}

#[test]
fn validates_stream_command_parameters() {
    let invalid_start = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 10,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 0,
            stream_id: 1,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
        })),
    };
    assert!(matches!(
        encode_control_frame(&invalid_start),
        Err(ControlFrameError::InvalidStartStream)
    ));

    let invalid_stop = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 11,
        payload: Some(envelope::Payload::StopStream(StopStream {
            media_session_id: 5,
            stream_id: 0,
        })),
    };
    assert!(matches!(
        encode_control_frame(&invalid_stop),
        Err(ControlFrameError::InvalidStopStream)
    ));
}
