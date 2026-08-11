#![no_main]

use libfuzzer_sys::fuzz_target;
use sonimesh_protocol::{Capability, Envelope, decode_control_frame, envelope, negotiate_hello};

const PUBLISHED_HELLO_FRAME: [u8; 28] = [
    0x00, 0x00, 0x00, 0x18, 0x08, 0x01, 0x10, 0x07, 0x52, 0x12, 0x0A, 0x01, 0x01, 0x12, 0x01, 0x02,
    0x1A, 0x0A, 0x4C, 0x61, 0x62, 0x20, 0x73, 0x65, 0x6E, 0x64, 0x65, 0x72,
];

/// Local policy negotiated against every decoded peer hello.
const LOCAL_CAPABILITIES: [Capability; 4] = [
    Capability::FrameDuration10Ms,
    Capability::FrameDuration20Ms,
    Capability::ManualAddress,
    Capability::NetworkDiagnostics,
];

fn negotiate_decoded(envelope: &Envelope) {
    if let Some(envelope::Payload::Hello(hello)) = envelope.payload.as_ref() {
        let _ = negotiate_hello(hello, &LOCAL_CAPABILITIES);
    }
}

fn decode_mutated<const N: usize>(seed: [u8; N], mutations: &[u8]) {
    let mut near_valid = seed;
    for (target, mutation) in near_valid.iter_mut().zip(mutations) {
        *target ^= mutation;
    }
    if let Ok(envelope) = decode_control_frame(&near_valid) {
        negotiate_decoded(&envelope);
    }
}

fuzz_target!(|data: &[u8]| {
    if let Ok(envelope) = decode_control_frame(data) {
        negotiate_decoded(&envelope);
    }
    decode_mutated(PUBLISHED_HELLO_FRAME, data);
});
