//! Wire and semantic boundary tests for length-prefixed control Protobuf frames.

use prost::Message;
use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlError, ControlFrameError, Envelope, ErrorCode,
    Hello, MAX_CONTROL_CAPABILITIES, MAX_CONTROL_DETAIL_BYTES, MAX_CONTROL_DISPLAY_NAME_BYTES,
    MAX_CONTROL_FRAME_BYTES, MAX_CONTROL_MEDIA_KEY_EPOCH, MAX_CONTROL_SUPPORTED_VERSIONS,
    StartStream, StopStream, StreamState, StreamStatus, decode_control_frame, encode_control_frame,
    envelope,
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
fn matches_published_epoch_bearing_start_vector() {
    let envelope = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 8,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 7,
            stream_id: 9,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: 0x2345,
        })),
    };

    assert_eq!(
        encode_control_frame(&envelope).unwrap(),
        [
            0x00, 0x00, 0x00, 0x15, 0x08, 0x01, 0x10, 0x08, 0x5A, 0x0F, 0x08, 0x07, 0x10, 0x09,
            0x18, 0x80, 0xF7, 0x02, 0x20, 0x02, 0x28, 0x14, 0x30, 0xC5, 0x46,
        ]
    );
}

#[test]
fn matches_published_epoch_bearing_stop_vector() {
    let envelope = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 9,
        payload: Some(envelope::Payload::StopStream(StopStream {
            media_session_id: 7,
            stream_id: 9,
            media_key_epoch: 0x2345,
        })),
    };

    assert_eq!(
        encode_control_frame(&envelope).unwrap(),
        [
            0x00, 0x00, 0x00, 0x0D, 0x08, 0x01, 0x10, 0x09, 0x62, 0x07, 0x08, 0x07, 0x10, 0x09,
            0x18, 0xC5, 0x46,
        ]
    );
}

#[test]
fn matches_published_stream_status_vector() {
    let envelope = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 10,
        payload: Some(envelope::Payload::StreamStatus(StreamStatus {
            media_session_id: 7,
            stream_id: 9,
            state: StreamState::Streaming as i32,
            detail: "receiving".to_owned(),
        })),
    };

    assert_eq!(
        encode_control_frame(&envelope).unwrap(),
        [
            0x00, 0x00, 0x00, 0x17, 0x08, 0x01, 0x10, 0x0A, 0x6A, 0x11, 0x08, 0x07, 0x10, 0x09,
            0x18, 0x02, 0x22, 0x09, 0x72, 0x65, 0x63, 0x65, 0x69, 0x76, 0x69, 0x6E, 0x67,
        ]
    );
}

#[test]
fn matches_published_control_error_vector() {
    let envelope = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 11,
        payload: Some(envelope::Payload::Error(ControlError {
            code: ErrorCode::RetryLater as i32,
            retryable: true,
            detail: "try again".to_owned(),
        })),
    };

    assert_eq!(
        encode_control_frame(&envelope).unwrap(),
        [
            0x00, 0x00, 0x00, 0x15, 0x08, 0x01, 0x10, 0x0B, 0x72, 0x0F, 0x08, 0x05, 0x10, 0x01,
            0x1A, 0x09, 0x74, 0x72, 0x79, 0x20, 0x61, 0x67, 0x61, 0x69, 0x6E,
        ]
    );
}

#[test]
fn round_trips_stream_commands_with_the_exact_key_epoch() {
    let start = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 21,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 0x0102_0304_0506_0708,
            stream_id: 9,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: 0xFFFF,
        })),
    };
    let stop = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 22,
        payload: Some(envelope::Payload::StopStream(StopStream {
            media_session_id: 0x0102_0304_0506_0708,
            stream_id: 9,
            media_key_epoch: 0xFFFF,
        })),
    };

    assert_eq!(
        decode_control_frame(&encode_control_frame(&start).unwrap()).unwrap(),
        start
    );
    assert_eq!(
        decode_control_frame(&encode_control_frame(&stop).unwrap()).unwrap(),
        stop
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
fn declared_length_at_maximum_capacity_is_not_frame_too_large() {
    // The capacity check must be strictly greater-than: a declared payload of
    // exactly MAX_CONTROL_FRAME_BYTES stays in bounds and fails later framing
    // checks, never with FrameTooLarge.
    let maximum = u32::try_from(MAX_CONTROL_FRAME_BYTES)
        .unwrap()
        .to_be_bytes();
    assert!(matches!(
        decode_control_frame(&maximum),
        Err(ControlFrameError::LengthMismatch)
    ));
}

#[test]
fn maximum_size_frame_reaches_validation_without_frame_too_large() {
    // A complete frame at the exact 64 KiB capacity must pass the capacity and
    // length checks and reach envelope validation instead of being rejected as
    // oversized. All-zero bytes decode to an empty envelope, which fails the
    // version check first.
    let mut frame = Vec::with_capacity(4 + MAX_CONTROL_FRAME_BYTES);
    frame.extend_from_slice(
        &u32::try_from(MAX_CONTROL_FRAME_BYTES)
            .unwrap()
            .to_be_bytes(),
    );
    frame.resize(4 + MAX_CONTROL_FRAME_BYTES, 0);
    // All-zero bytes begin a protobuf field but the following zero tag is an
    // invalid field number, so the bounded decode fails without a capacity or
    // length error.
    assert!(matches!(
        decode_control_frame(&frame),
        Err(ControlFrameError::Decode(_))
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
            code: ErrorCode::InvalidRequest as i32,
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
fn validates_hello_bounds_on_encode_and_untrusted_decode_paths() {
    let maximum = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 15,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION; MAX_CONTROL_SUPPORTED_VERSIONS],
            capabilities: vec![Capability::FrameDuration20Ms as i32; MAX_CONTROL_CAPABILITIES],
            display_name: "x".repeat(MAX_CONTROL_DISPLAY_NAME_BYTES),
        })),
    };
    assert_eq!(
        decode_control_frame(&encode_control_frame(&maximum).unwrap()).unwrap(),
        maximum
    );

    let too_many_versions = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 16,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION; MAX_CONTROL_SUPPORTED_VERSIONS + 1],
            capabilities: Vec::new(),
            display_name: String::new(),
        })),
    };
    assert!(matches!(
        encode_control_frame(&too_many_versions),
        Err(ControlFrameError::TooManySupportedVersions)
    ));
    assert!(matches!(
        decode_control_frame(&raw_frame(&too_many_versions)),
        Err(ControlFrameError::TooManySupportedVersions)
    ));

    let too_many_capabilities = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 17,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: vec![0; MAX_CONTROL_CAPABILITIES + 1],
            display_name: String::new(),
        })),
    };
    assert!(matches!(
        encode_control_frame(&too_many_capabilities),
        Err(ControlFrameError::TooManyCapabilities)
    ));
    assert!(matches!(
        decode_control_frame(&raw_frame(&too_many_capabilities)),
        Err(ControlFrameError::TooManyCapabilities)
    ));

    let display_name_too_long = "é".repeat(MAX_CONTROL_DISPLAY_NAME_BYTES / "é".len() + 1);
    assert!(display_name_too_long.len() > MAX_CONTROL_DISPLAY_NAME_BYTES);
    assert!(display_name_too_long.chars().count() <= MAX_CONTROL_DISPLAY_NAME_BYTES);
    let display_name_too_long = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 18,
        payload: Some(envelope::Payload::Hello(Hello {
            supported_versions: vec![CONTROL_PROTOCOL_VERSION],
            capabilities: Vec::new(),
            display_name: display_name_too_long,
        })),
    };
    assert!(matches!(
        encode_control_frame(&display_name_too_long),
        Err(ControlFrameError::DisplayNameTooLong)
    ));
    assert!(matches!(
        decode_control_frame(&raw_frame(&display_name_too_long)),
        Err(ControlFrameError::DisplayNameTooLong)
    ));
}

#[test]
fn validates_stream_status_and_control_error_semantics() {
    let valid_status = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 10,
        payload: Some(envelope::Payload::StreamStatus(StreamStatus {
            media_session_id: 5,
            stream_id: 6,
            state: StreamState::Streaming as i32,
            detail: "receiving".to_owned(),
        })),
    };
    assert_eq!(
        decode_control_frame(&encode_control_frame(&valid_status).unwrap()).unwrap(),
        valid_status
    );

    for (media_session_id, stream_id) in [(0, 6), (5, 0)] {
        let invalid_identity = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 11,
            payload: Some(envelope::Payload::StreamStatus(StreamStatus {
                media_session_id,
                stream_id,
                state: StreamState::Ready as i32,
                detail: String::new(),
            })),
        };
        assert!(matches!(
            encode_control_frame(&invalid_identity),
            Err(ControlFrameError::InvalidStreamStatusIdentity)
        ));
        assert!(matches!(
            decode_control_frame(&raw_frame(&invalid_identity)),
            Err(ControlFrameError::InvalidStreamStatusIdentity)
        ));
    }

    for state in [StreamState::Unspecified as i32, 99] {
        let invalid_state = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 12,
            payload: Some(envelope::Payload::StreamStatus(StreamStatus {
                media_session_id: 5,
                stream_id: 6,
                state,
                detail: String::new(),
            })),
        };
        assert!(matches!(
            encode_control_frame(&invalid_state),
            Err(ControlFrameError::InvalidStreamState(value)) if value == state
        ));
        assert!(matches!(
            decode_control_frame(&raw_frame(&invalid_state)),
            Err(ControlFrameError::InvalidStreamState(value)) if value == state
        ));
    }

    let valid_error = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 13,
        payload: Some(envelope::Payload::Error(ControlError {
            code: ErrorCode::RetryLater as i32,
            retryable: true,
            detail: "try again".to_owned(),
        })),
    };
    assert_eq!(
        decode_control_frame(&encode_control_frame(&valid_error).unwrap()).unwrap(),
        valid_error
    );

    for code in [ErrorCode::Unspecified as i32, 99] {
        let invalid_error = Envelope {
            protocol_version: CONTROL_PROTOCOL_VERSION,
            request_id: 14,
            payload: Some(envelope::Payload::Error(ControlError {
                code,
                retryable: false,
                detail: String::new(),
            })),
        };
        assert!(matches!(
            encode_control_frame(&invalid_error),
            Err(ControlFrameError::InvalidErrorCode(value)) if value == code
        ));
        assert!(matches!(
            decode_control_frame(&raw_frame(&invalid_error)),
            Err(ControlFrameError::InvalidErrorCode(value)) if value == code
        ));
    }
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
            media_key_epoch: 0,
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
            media_key_epoch: 0,
        })),
    };
    assert!(matches!(
        encode_control_frame(&invalid_stop),
        Err(ControlFrameError::InvalidStopStream)
    ));

    let invalid_epoch = Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: 12,
        payload: Some(envelope::Payload::StartStream(StartStream {
            media_session_id: 5,
            stream_id: 6,
            sample_rate_hz: 48_000,
            channels: 2,
            frame_duration_ms: 20,
            media_key_epoch: MAX_CONTROL_MEDIA_KEY_EPOCH + 1,
        })),
    };
    assert!(matches!(
        encode_control_frame(&invalid_epoch),
        Err(ControlFrameError::InvalidMediaKeyEpoch)
    ));
}
