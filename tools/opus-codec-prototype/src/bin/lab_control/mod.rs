use std::io::{self, Read};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
    mpsc::{SyncSender, TrySendError},
};

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

#[allow(dead_code)] // The standalone UDP lab uses only gain controls.
pub(crate) struct OutputRequest {
    pub(crate) id: u64,
    pub(crate) session: u64,
    pub(crate) device: Option<String>,
    pub(crate) deadline: std::time::Instant,
}

fn output_request(
    line: &str,
    input: &mut impl Read,
) -> Result<Option<OutputRequest>, &'static str> {
    let mut words = line.split_whitespace();
    if words.next() != Some("output") {
        return Ok(None);
    }
    let id = words
        .next()
        .ok_or("invalid output request")?
        .parse()
        .map_err(|_| "invalid output request")?;
    let session = words
        .next()
        .ok_or("invalid output request")?
        .parse()
        .map_err(|_| "invalid output request")?;
    let length: usize = words
        .next()
        .ok_or("invalid output request")?
        .parse()
        .map_err(|_| "invalid output request")?;
    if words.next().is_some() || length > 4096 {
        return Err("invalid output request");
    }
    let mut bytes = vec![0; length];
    input
        .read_exact(&mut bytes)
        .map_err(|_| "incomplete output request")?;
    let device = if length == 0 {
        None
    } else {
        Some(String::from_utf8(bytes).map_err(|_| "invalid output request")?)
    };
    Ok(Some(OutputRequest {
        id,
        session,
        device,
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(10),
    }))
}

#[allow(dead_code)]
pub(crate) fn start_console_control(gain: GainControlHandle) {
    let (requests, _) = std::sync::mpsc::sync_channel(1);
    start_console_control_with_output(gain, requests, Arc::new(AtomicU64::new(0)));
}

#[allow(dead_code)]
pub(crate) fn start_console_control_with_output(
    gain: GainControlHandle,
    requests: SyncSender<OutputRequest>,
    session: Arc<AtomicU64>,
) {
    std::thread::spawn(move || {
        println!("Audio controls: volume 0-100 | mute | unmute");
        println!(
            "CONTROL volume {:.0} muted {}",
            gain.gain() * 100.0,
            gain.is_muted()
        );
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
                            .and_then(|line| {
                                if let Some(request) = output_request(line, &mut input)? {
                                    if request.session == 0
                                        || request.session != session.load(Ordering::Acquire)
                                    {
                                        println!("OUTPUT-SWITCH {} inactive", request.id);
                                    } else if let Err(error) = requests.try_send(request) {
                                        let request = match error {
                                            TrySendError::Full(request)
                                            | TrySendError::Disconnected(request) => request,
                                        };
                                        println!("OUTPUT-SWITCH {} busy", request.id);
                                    }
                                    return Ok(false);
                                }
                                apply_control(line, &gain).map(|()| true)
                            })
                    };
                    match result {
                        Ok(true) => println!(
                            "CONTROL volume {:.0} muted {}",
                            gain.gain() * 100.0,
                            gain.is_muted()
                        ),
                        Ok(false) => {}
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_request_reads_only_the_framed_device_id() {
        let mut input = &b"a\nbmute\n"[..];
        let request = output_request("output 42 7 3", &mut input)
            .unwrap()
            .unwrap();
        assert_eq!(
            (request.id, request.session, request.device.as_deref()),
            (42, 7, Some("a\nb"))
        );
        assert_eq!(input, b"mute\n");
        assert!(output_request("output 3 7 4097", &mut input).is_err());
        assert!(output_request("output 3 7 9", &mut input).is_err());
        assert!(output_request("mute", &mut input).unwrap().is_none());
        assert!(
            output_request("output 9 7 0", &mut input)
                .unwrap()
                .unwrap()
                .device
                .is_none()
        );
    }
}
