//! Decode the synthetic fixture produced by the isolated Android codec spike.
use std::{error::Error, fs::File, io::Read};

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args().nth(1).ok_or("expected fixture path")?;
    let mut file = File::open(path)?;
    let mut decoder = opus::Decoder::new(48_000, opus::Channels::Stereo)?;
    let mut packet = [0_u8; 1_148];
    let mut pcm = vec![0_f32; 5_760 * 2];
    let mut peak = 0_f32;
    let mut count = 0_u32;
    let mut samples = 0_usize;
    loop {
        let mut prefix = [0_u8; 4];
        match file.read(&mut prefix[..1])? {
            0 => break,
            1 => file.read_exact(&mut prefix[1..])?,
            _ => unreachable!(),
        }
        let len = u32::from_be_bytes(prefix) as usize;
        if len == 0 || len > packet.len() || count >= 200 {
            return Err("invalid fixture packet length or count".into());
        }
        file.read_exact(&mut packet[..len])?;
        let frames = decoder.decode_float(&packet[..len], &mut pcm, false)?;
        if frames != 960 {
            return Err("fixture packet is not 20 ms at 48 kHz".into());
        }
        for &value in &pcm[..frames * 2] {
            if !value.is_finite() {
                return Err("non-finite decoded sample".into());
            }
            peak = peak.max(value.abs());
        }
        count += 1;
        samples += frames;
    }
    if count != 100 || peak < 0.001 {
        return Err("fixture did not decode to 100 non-silent frames".into());
    }
    println!("decoded {count} Android Opus packets, {samples} samples/channel, peak {peak:.3}");
    Ok(())
}
