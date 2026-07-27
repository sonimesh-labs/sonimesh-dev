#![no_main]

use libfuzzer_sys::fuzz_target;
use sonimesh_protocol::decode_control_frame;

const PUBLISHED_HELLO_FRAME: [u8; 28] = [
    0x00, 0x00, 0x00, 0x18, 0x08, 0x01, 0x10, 0x07, 0x52, 0x12, 0x0A, 0x01, 0x01, 0x12, 0x01, 0x02,
    0x1A, 0x0A, 0x4C, 0x61, 0x62, 0x20, 0x73, 0x65, 0x6E, 0x64, 0x65, 0x72,
];

fuzz_target!(|data: &[u8]| {
    let _ = decode_control_frame(data);

    let mut near_valid = PUBLISHED_HELLO_FRAME;
    for (target, mutation) in near_valid.iter_mut().zip(data) {
        *target ^= mutation;
    }
    let _ = decode_control_frame(&near_valid);
});
