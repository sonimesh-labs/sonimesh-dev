use std::io::{self, Read};

use sonimesh_audio::GainControlHandle;

pub(crate) fn apply_control(input: &str, gain: &GainControlHandle) -> Result<(), &'static str> {
    let mut words = input.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (Some("mute"), None, None) => gain.set_muted(true),
        (Some("unmute"), None, None) => gain.set_muted(false),
        (Some("volume"), Some(value), None) => {
            let percent: u8 = value.parse().map_err(|_| "volume must be 0 to 100")?;
            if percent > 100 {
                return Err("volume must be 0 to 100");
            }
            gain.set_gain(f32::from(percent) / 100.0)
                .map_err(|_| "invalid volume")?;
        }
        _ => return Err("use: volume 0-100 | mute | unmute"),
    }
    Ok(())
}

#[cfg_attr(not(all(windows, feature = "playback")), allow(dead_code))]
pub(crate) fn start_console_control(gain: GainControlHandle) {
    std::thread::spawn(move || {
        println!("Audio controls: volume 0-100 | mute | unmute");
        println!("CONTROL volume 100 muted false");
        let mut input = io::stdin().lock();
        let mut buffer = [0_u8; 64];
        let mut length = 0;
        let mut overflow = false;
        loop {
            let mut byte = [0_u8; 1];
            match input.read(&mut byte) {
                Ok(0) => break,
                Ok(_) if byte[0] == b'\n' => {
                    let result = if overflow {
                        Err("command too long")
                    } else {
                        std::str::from_utf8(&buffer[..length])
                            .map_err(|_| "invalid command")
                            .and_then(|line| apply_control(line, &gain))
                    };
                    match result {
                        Ok(()) => println!(
                            "CONTROL volume {:.0} muted {}",
                            gain.gain() * 100.0,
                            gain.is_muted()
                        ),
                        Err(error) => eprintln!("{error}"),
                    }
                    length = 0;
                    overflow = false;
                }
                Ok(_) if length < buffer.len() && !overflow => {
                    buffer[length] = byte[0];
                    length += 1;
                }
                Ok(_) => overflow = true,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    eprintln!("Audio controls unavailable: {error}");
                    break;
                }
            }
        }
    });
}
