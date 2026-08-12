//! Deterministic property coverage for the control frame wire boundary.
//!
//! `encode_control_frame` and `decode_control_frame` must agree with the public
//! validator: an envelope encodes exactly when it validates, the encoded frame
//! decodes back to the identical envelope with byte-exact canonical bytes, and
//! the framing boundary rejects every deterministic mutation of a valid frame
//! (appended tail bytes, truncation, or a corrupted length prefix) before any
//! Protobuf work. This test walks a seeded random envelope generator and then
//! mutates freshly encoded frames, so both the encode/decode round trip and
//! the strict framing contract are exercised across the bounded field domain.

use sonimesh_protocol::{
    CONTROL_PROTOCOL_VERSION, Capability, ControlError, Envelope, ErrorCode, Hello,
    MAX_CONTROL_CAPABILITIES, MAX_CONTROL_CHANNELS, MAX_CONTROL_DETAIL_BYTES,
    MAX_CONTROL_DISPLAY_NAME_BYTES, MAX_CONTROL_FRAME_BYTES, MAX_CONTROL_MEDIA_KEY_EPOCH,
    MAX_CONTROL_SAMPLE_RATE_HZ, MAX_CONTROL_SUPPORTED_VERSIONS, MIN_CONTROL_SAMPLE_RATE_HZ,
    StartStream, StopStream, StreamState, StreamStatus, decode_control_frame, encode_control_frame,
    envelope, validate_control_envelope,
};

/// Small deterministic xorshift64 generator; no external dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }

    fn pick<'a, T>(&mut self, choices: &'a [T]) -> &'a T {
        &choices[self.next_usize(choices.len())]
    }
}

fn random_string(rng: &mut XorShift64, max_bytes: usize) -> String {
    let len = rng.next_usize(max_bytes + 1);
    let mut bytes = Vec::with_capacity(len);
    for _ in 0..len {
        bytes.push(u8::try_from(rng.next_u32() % 0x5E).expect("bounded by 0x5E") + 0x20);
    }
    String::from_utf8(bytes).expect("generated ASCII is valid UTF-8")
}

fn nonzero_id(rng: &mut XorShift64) -> u64 {
    rng.next_u64().max(1)
}

fn nonzero_id32(rng: &mut XorShift64) -> u32 {
    rng.next_u32().max(1)
}

fn random_hello(rng: &mut XorShift64) -> Hello {
    let versions_len = rng.next_usize(MAX_CONTROL_SUPPORTED_VERSIONS + 3);
    let capabilities_len = rng.next_usize(MAX_CONTROL_CAPABILITIES + 3);
    Hello {
        supported_versions: (0..versions_len)
            .map(|_| *rng.pick(&[1, CONTROL_PROTOCOL_VERSION, 0, 2, u32::MAX]))
            .collect(),
        capabilities: (0..capabilities_len)
            .map(|_| *rng.pick(&[0_i32, 1, 2, 3, 4, 99, 255]))
            .collect(),
        display_name: random_string(rng, MAX_CONTROL_DISPLAY_NAME_BYTES + 3),
    }
}

fn random_start(rng: &mut XorShift64) -> StartStream {
    StartStream {
        media_session_id: nonzero_id(rng),
        stream_id: nonzero_id32(rng),
        sample_rate_hz: *rng.pick(&[
            MIN_CONTROL_SAMPLE_RATE_HZ,
            44_100,
            48_000,
            MAX_CONTROL_SAMPLE_RATE_HZ,
            MIN_CONTROL_SAMPLE_RATE_HZ - 1,
            MAX_CONTROL_SAMPLE_RATE_HZ + 1,
            0,
        ]),
        channels: *rng.pick(&[1, 2, MAX_CONTROL_CHANNELS, 0, 9]),
        frame_duration_ms: *rng.pick(&[10, 20, 0, 15]),
        media_key_epoch: *rng.pick(&[
            0,
            0x2345,
            MAX_CONTROL_MEDIA_KEY_EPOCH,
            MAX_CONTROL_MEDIA_KEY_EPOCH + 1,
        ]),
    }
}

fn random_stop(rng: &mut XorShift64) -> StopStream {
    StopStream {
        media_session_id: nonzero_id(rng),
        stream_id: nonzero_id32(rng),
        media_key_epoch: *rng.pick(&[
            0,
            0x2345,
            MAX_CONTROL_MEDIA_KEY_EPOCH,
            MAX_CONTROL_MEDIA_KEY_EPOCH + 1,
        ]),
    }
}

fn random_status(rng: &mut XorShift64) -> StreamStatus {
    StreamStatus {
        media_session_id: nonzero_id(rng),
        stream_id: nonzero_id32(rng),
        state: *rng.pick(&[
            StreamState::Ready as i32,
            StreamState::Streaming as i32,
            StreamState::Reconnecting as i32,
            StreamState::Stopped as i32,
            StreamState::Failed as i32,
            0,
            99,
        ]),
        detail: random_string(rng, MAX_CONTROL_DETAIL_BYTES + 3),
    }
}

fn random_error(rng: &mut XorShift64) -> ControlError {
    ControlError {
        code: *rng.pick(&[
            ErrorCode::IncompatibleVersion as i32,
            ErrorCode::UnsupportedCapability as i32,
            ErrorCode::InvalidRequest as i32,
            ErrorCode::StreamNotFound as i32,
            ErrorCode::RetryLater as i32,
            0,
            99,
        ]),
        retryable: rng.next_u32() % 2 == 0,
        detail: random_string(rng, MAX_CONTROL_DETAIL_BYTES + 3),
    }
}

/// Random envelope whose fields cross every validated bound in both directions.
fn random_envelope(rng: &mut XorShift64) -> Envelope {
    Envelope {
        protocol_version: *rng.pick(&[CONTROL_PROTOCOL_VERSION, 0, 2, u32::MAX]),
        request_id: rng.next_u64(),
        payload: match rng.next_usize(6) {
            0 => Some(envelope::Payload::Hello(random_hello(rng))),
            1 => Some(envelope::Payload::StartStream(random_start(rng))),
            2 => Some(envelope::Payload::StopStream(random_stop(rng))),
            3 => Some(envelope::Payload::StreamStatus(random_status(rng))),
            4 => Some(envelope::Payload::Error(random_error(rng))),
            _ => None,
        },
    }
}

/// Random envelope that always satisfies version-one validation.
fn random_valid_envelope(rng: &mut XorShift64) -> Envelope {
    Envelope {
        protocol_version: CONTROL_PROTOCOL_VERSION,
        request_id: rng.next_u64(),
        payload: Some(match rng.next_usize(5) {
            0 => envelope::Payload::Hello(Hello {
                supported_versions: vec![CONTROL_PROTOCOL_VERSION],
                capabilities: vec![Capability::FrameDuration20Ms as i32],
                display_name: random_string(rng, MAX_CONTROL_DISPLAY_NAME_BYTES),
            }),
            1 => envelope::Payload::StartStream(StartStream {
                media_session_id: nonzero_id(rng),
                stream_id: nonzero_id32(rng),
                sample_rate_hz: *rng.pick(&[
                    MIN_CONTROL_SAMPLE_RATE_HZ,
                    44_100,
                    48_000,
                    MAX_CONTROL_SAMPLE_RATE_HZ,
                ]),
                channels: *rng.pick(&[1, 2, MAX_CONTROL_CHANNELS]),
                frame_duration_ms: *rng.pick(&[10, 20]),
                media_key_epoch: *rng.pick(&[0, 0x2345, MAX_CONTROL_MEDIA_KEY_EPOCH]),
            }),
            2 => envelope::Payload::StopStream(StopStream {
                media_session_id: nonzero_id(rng),
                stream_id: nonzero_id32(rng),
                media_key_epoch: *rng.pick(&[0, 0x2345, MAX_CONTROL_MEDIA_KEY_EPOCH]),
            }),
            3 => envelope::Payload::StreamStatus(StreamStatus {
                media_session_id: nonzero_id(rng),
                stream_id: nonzero_id32(rng),
                state: *rng.pick(&[
                    StreamState::Ready as i32,
                    StreamState::Streaming as i32,
                    StreamState::Reconnecting as i32,
                    StreamState::Stopped as i32,
                    StreamState::Failed as i32,
                ]),
                detail: random_string(rng, MAX_CONTROL_DETAIL_BYTES),
            }),
            _ => envelope::Payload::Error(ControlError {
                code: *rng.pick(&[
                    ErrorCode::IncompatibleVersion as i32,
                    ErrorCode::UnsupportedCapability as i32,
                    ErrorCode::InvalidRequest as i32,
                    ErrorCode::StreamNotFound as i32,
                    ErrorCode::RetryLater as i32,
                ]),
                retryable: rng.next_u32() % 2 == 0,
                detail: random_string(rng, MAX_CONTROL_DETAIL_BYTES),
            }),
        }),
    }
}

#[test]
fn randomized_envelopes_encode_exactly_when_they_validate_and_round_trip_canonically() {
    const STEPS: usize = 5_000;
    let mut rng = XorShift64(0xC0DE_0000_2026_0016);
    let mut observed_valid = false;
    let mut observed_invalid = false;
    let mut observed_kinds = [false; 5];

    for _ in 0..STEPS {
        let envelope = random_envelope(&mut rng);
        if let Some(payload) = &envelope.payload {
            let kind = match payload {
                envelope::Payload::Hello(_) => 0,
                envelope::Payload::StartStream(_) => 1,
                envelope::Payload::StopStream(_) => 2,
                envelope::Payload::StreamStatus(_) => 3,
                envelope::Payload::Error(_) => 4,
            };
            observed_kinds[kind] = true;
        }

        let validates = validate_control_envelope(&envelope).is_ok();
        let encoded = encode_control_frame(&envelope);
        assert_eq!(
            encoded.is_ok(),
            validates,
            "encode acceptance must match public validation for {envelope:?}"
        );
        if validates {
            observed_valid = true;
            let frame = encoded.expect("validated envelope encodes");
            assert!(
                frame.len() <= 4 + MAX_CONTROL_FRAME_BYTES,
                "frame must respect the hard limit"
            );
            let decoded = decode_control_frame(&frame).expect("valid frame decodes");
            assert_eq!(decoded, envelope, "decode must restore the exact envelope");
            assert_eq!(
                encode_control_frame(&decoded).expect("decoded envelope stays valid"),
                frame,
                "canonical re-encode must reproduce the frame byte-for-byte"
            );
        } else {
            observed_invalid = true;
        }
    }

    assert!(observed_valid, "the walk must generate valid envelopes");
    assert!(observed_invalid, "the walk must generate invalid envelopes");
    assert!(
        observed_kinds.iter().all(|seen| *seen),
        "every payload kind must be exercised"
    );
}

#[test]
fn deterministic_frame_mutations_are_rejected_at_the_framing_boundary() {
    const FRAMES: usize = 500;
    let mut rng = XorShift64(0xC0DE_0000_2026_0017);

    for _ in 0..FRAMES {
        let envelope = random_valid_envelope(&mut rng);
        let frame = encode_control_frame(&envelope).expect("valid envelope encodes");
        assert_eq!(
            decode_control_frame(&frame).unwrap(),
            envelope,
            "sanity: the untouched frame decodes"
        );

        // Appended trailing bytes must fail the exact-length check.
        for extra in 1..=8 {
            let mut mutated = frame.clone();
            let tail: Vec<u8> = (0..extra)
                .map(|_| u8::try_from(rng.next_u32() % 256).expect("bounded by 256"))
                .collect();
            mutated.extend(tail);
            assert!(
                decode_control_frame(&mutated).is_err(),
                "appended trailing bytes must be rejected"
            );
        }

        // Truncation at every tail offset must be rejected.
        for remove in 1..=frame.len().min(8) {
            assert!(
                decode_control_frame(&frame[..frame.len() - remove]).is_err(),
                "truncated frame must be rejected"
            );
        }

        // Corrupt the big-endian length prefix: one too large, byte-swapped,
        // and zero. All must be rejected before Protobuf work.
        let payload_len = u32::try_from(frame.len() - 4).expect("frame length fits u32");
        for declared in [payload_len + 1, payload_len.swap_bytes(), 0] {
            let mut mutated = frame.clone();
            mutated[..4].copy_from_slice(&declared.to_be_bytes());
            assert!(
                decode_control_frame(&mutated).is_err(),
                "corrupt length prefix {declared} must be rejected"
            );
        }
    }
}
