//! Deterministic adversarial byte regression while cargo-fuzz is unavailable.

use sonimesh_protocol::{
    MAX_CONTROL_FRAME_BYTES, MAX_MEDIA_DATAGRAM_LEN, decode_control_frame, parse_media_packet,
};

struct DeterministicBytes(u64);

impl DeterministicBytes {
    fn fill(&mut self, bytes: &mut [u8]) {
        for byte in bytes {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            *byte = self.0.to_le_bytes()[4];
        }
    }

    fn length(&mut self, maximum: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        usize::try_from(self.0 % u64::try_from(maximum + 1).unwrap()).unwrap()
    }
}

#[test]
fn bounded_arbitrary_control_and_media_bytes_never_panic() {
    let mut random = DeterministicBytes(0xC0DE_2026_A11C_E5ED);

    for _ in 0..1_024 {
        let mut control = vec![0_u8; random.length(MAX_CONTROL_FRAME_BYTES + 512)];
        random.fill(&mut control);
        let _ = decode_control_frame(&control);

        let mut media = vec![0_u8; random.length(MAX_MEDIA_DATAGRAM_LEN + 64)];
        random.fill(&mut media);
        let _ = parse_media_packet(&media);
    }
}
