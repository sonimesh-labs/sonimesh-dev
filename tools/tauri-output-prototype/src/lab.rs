use serde::Serialize;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Ipv4Addr;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabSnapshot {
    pub running: bool,
    pub status: String,
    pub invitation: Option<String>,
    pub metrics: Option<LabMetrics>,
    pub volume: Option<u8>,
    pub muted: Option<bool>,
    #[serde(skip)]
    session_active: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabMetrics {
    packets: String,
    rejected: String,
    decoded: String,
    peak: String,
    plc: String,
    decode_errors: String,
    buffered: String,
    output: String,
}

impl Default for LabSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            status: "Stopped".into(),
            invitation: None,
            metrics: None,
            volume: None,
            muted: None,
            session_active: false,
        }
    }
}

#[derive(Default)]
pub struct Lab {
    child: Option<Child>,
    events: Option<Receiver<String>>,
    snapshot: LabSnapshot,
}

impl Lab {
    pub fn start(&mut self, host: &str, output_id: Option<&str>) -> Result<LabSnapshot, String> {
        self.refresh();
        if self.child.is_some() {
            return Err("The listener is already running.".into());
        }
        let host: Ipv4Addr = host.parse().map_err(|_| "Enter a valid IPv4 address.")?;
        if host.is_unspecified() || host.is_multicast() || host.is_broadcast() {
            return Err("Enter a unicast IPv4 address.".into());
        }
        let executable = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/debug/paired-audio-lab.exe");
        let mut command = Command::new(executable);
        command
            .args([
                "--play",
                "--keep-listening",
                "--status",
                "--control",
                "--bind",
                "0.0.0.0",
                "--host",
                &host.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if output_id.is_some() {
            command.arg("--output-id-framed");
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(
            |_| "Could not launch the lab listener. Build it with playback support first.",
        )?;
        if let Some(id) = output_id {
            let result = u32::try_from(id.len())
                .ok()
                .filter(|size| *size > 0 && *size <= 4096)
                .ok_or(())
                .and_then(|size| {
                    let stdin = child.stdin.as_mut().expect("piped stdin");
                    stdin
                        .write_all(&size.to_be_bytes())
                        .and_then(|()| stdin.write_all(id.as_bytes()))
                        .map_err(|_| ())
                });
            if result.is_err() {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Could not send the output selection to the listener.".into());
            }
        }
        let (sender, events) = mpsc::sync_channel(128);
        read_output(child.stdout.take().expect("piped stdout"), sender.clone());
        read_output(child.stderr.take().expect("piped stderr"), sender);
        self.child = Some(child);
        self.events = Some(events);
        self.snapshot = LabSnapshot {
            running: true,
            status: "Starting listener…".into(),
            invitation: None,
            metrics: None,
            volume: None,
            muted: None,
            session_active: false,
        };
        Ok(self.snapshot.clone())
    }

    pub fn refresh(&mut self) -> LabSnapshot {
        if let Some(events) = &self.events {
            for line in events.try_iter().take(128) {
                update_snapshot(&mut self.snapshot, &line);
            }
        }
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.snapshot.running = false;
                    self.snapshot.invitation = None;
                    self.snapshot.metrics = None;
                    self.snapshot.volume = None;
                    self.snapshot.muted = None;
                    self.snapshot.session_active = false;
                    self.snapshot.status = "Listener exited. Check the selected output device and whether port 47610 is in use.".into();
                    self.child = None;
                    self.events = None;
                }
                Err(_) => self.snapshot.status = "Could not check the listener process.".into(),
                Ok(None) => {}
            }
        }
        self.snapshot.clone()
    }

    pub fn stop(&mut self) -> LabSnapshot {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.events = None;
        self.snapshot = LabSnapshot::default();
        self.snapshot.clone()
    }

    pub fn control(&mut self, volume: Option<u8>, muted: Option<bool>) -> Result<(), &'static str> {
        let command = match (volume, muted) {
            (Some(value @ 0..=100), None) => format!("volume {value}\n"),
            (None, Some(true)) => "mute\n".into(),
            (None, Some(false)) => "unmute\n".into(),
            _ => return Err("Invalid audio control."),
        };
        self.refresh();
        let child = self.child.as_mut().ok_or("Listener is not running.")?;
        child
            .stdin
            .as_mut()
            .ok_or("Audio control is unavailable.")?
            .write_all(command.as_bytes())
            .map_err(|_| "Could not send audio control.")
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read_output(output: impl Read + Send + 'static, sender: SyncSender<String>) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(output);
        let mut line = Vec::with_capacity(4096);
        let mut oversized = false;
        while let Ok(bytes) = reader.fill_buf() {
            if bytes.is_empty() {
                break;
            }
            let consumed = bytes
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |index| index + 1);
            let part = &bytes[..consumed];
            if line.len() + part.len() <= 4096 {
                line.extend_from_slice(part);
            } else {
                oversized = true;
            }
            if part.last() == Some(&b'\n') {
                if !oversized {
                    let text = String::from_utf8_lossy(&line);
                    let _ = sender.try_send(text.trim_end_matches(['\r', '\n']).to_owned());
                }
                line.clear();
                oversized = false;
            }
            reader.consume(consumed);
        }
    });
}

fn update_snapshot(snapshot: &mut LabSnapshot, line: &str) {
    if let ["CONTROL", "volume", volume, "muted", muted] =
        line.split_whitespace().collect::<Vec<_>>().as_slice()
    {
        if let (Ok(volume @ 0..=100), Ok(muted)) = (volume.parse::<u8>(), muted.parse::<bool>()) {
            snapshot.volume = Some(volume);
            snapshot.muted = Some(muted);
        }
    } else if let Some(invitation) = line.strip_prefix("INVITATION_HEX ") {
        if invitation.len() <= 1024 && invitation.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            snapshot.invitation = Some(invitation.to_owned());
        }
    } else if line == "READY" {
        snapshot.status = "Waiting for Android".into();
        snapshot.metrics = None;
        snapshot.session_active = false;
    } else if line == "RESULT pairing verified" {
        snapshot.status = "Paired; negotiating session".into();
    } else if line == "RESULT receiver started" {
        snapshot.status = "Session active".into();
        snapshot.metrics = None;
        snapshot.session_active = true;
    } else if snapshot.session_active
        && let Some(metrics) = parse_metrics(line)
    {
        snapshot.metrics = Some(metrics);
    } else if line.starts_with("Session ended:") && !snapshot.session_active {
        snapshot.status = "Session ended; waiting for Android".into();
    }
}

fn parse_metrics(line: &str) -> Option<LabMetrics> {
    let parts: Vec<_> = line.split_whitespace().collect();
    let [
        "STATUS",
        "packets",
        packets,
        "rejected",
        rejected,
        "decoded",
        decoded,
        "peak",
        peak,
        "plc",
        plc,
        "decode_errors",
        decode_errors,
        "buffered",
        buffered,
        "output",
        output,
    ] = parts.as_slice()
    else {
        return None;
    };
    for value in [packets, rejected, decoded, plc, decode_errors, buffered] {
        value.parse::<u64>().ok()?;
    }
    let value: f32 = peak.parse().ok()?;
    if !value.is_finite()
        || value < 0.0
        || !matches!(*output, "active" | "recovering" | "software-check")
    {
        return None;
    }
    Some(LabMetrics {
        packets: (*packets).into(),
        rejected: (*rejected).into(),
        decoded: (*decoded).into(),
        peak: (*peak).into(),
        plc: (*plc).into(),
        decode_errors: (*decode_errors).into(),
        buffered: (*buffered).into(),
        output: (*output).into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_tracks_sessions_and_accepts_only_bounded_hex_invitations() {
        let mut snapshot = LabSnapshot::default();
        update_snapshot(&mut snapshot, "INVITATION_HEX 01ab");
        update_snapshot(&mut snapshot, "READY");
        assert_eq!(snapshot.invitation.as_deref(), Some("01ab"));
        assert_eq!(snapshot.status, "Waiting for Android");
        update_snapshot(&mut snapshot, "RESULT receiver started");
        assert!(snapshot.status.contains("Session active"));
        update_snapshot(&mut snapshot, "INVITATION_HEX invalid");
        assert_eq!(snapshot.invitation.as_deref(), Some("01ab"));
    }

    #[test]
    fn rejects_invalid_host_before_launch() {
        let mut lab = Lab::default();
        for host in ["invalid", "0.0.0.0", "224.0.0.1", "255.255.255.255"] {
            assert!(lab.start(host, None).is_err());
        }
        assert!(!lab.refresh().running);
        assert!(lab.control(Some(101), None).is_err());
        assert!(lab.control(None, None).is_err());
        assert!(lab.control(Some(50), Some(true)).is_err());
        assert!(lab.control(Some(50), None).is_err());
    }

    #[test]
    fn metrics_follow_session_lifecycle_and_ignore_invalid_status() {
        let mut snapshot = LabSnapshot::default();
        let status = "STATUS packets 120 rejected 3 decoded 115 peak 0.250 plc 2 decode_errors 1 buffered 4 output recovering";
        update_snapshot(&mut snapshot, status);
        assert!(snapshot.metrics.is_none());
        update_snapshot(&mut snapshot, "RESULT receiver started");
        update_snapshot(&mut snapshot, status);
        let metrics = snapshot.metrics.clone().unwrap();
        assert_eq!(metrics.packets, "120");
        assert_eq!(metrics.output, "recovering");
        update_snapshot(&mut snapshot, "STATUS packets invalid");
        assert_eq!(snapshot.metrics, Some(metrics));
        update_snapshot(&mut snapshot, "READY");
        assert!(snapshot.metrics.is_none());
        update_snapshot(&mut snapshot, status);
        assert!(snapshot.metrics.is_none());
        assert!(parse_metrics(&status.replace("0.250", "NaN")).is_none());
        assert!(parse_metrics(&status.replace("recovering", "unknown")).is_none());
    }

    #[test]
    #[ignore = "requires the playback lab binary, a Windows output and free lab ports"]
    fn real_listener_starts_and_stops() {
        use std::time::{Duration, Instant};
        let output = sonimesh_platform::CpalOutput::new();
        let device = output.default_device_probe().unwrap().device.id;
        let mut lab = Lab::default();
        lab.start("127.0.0.1", Some(device.as_str())).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let snapshot = loop {
            let snapshot = lab.refresh();
            if !snapshot.running || snapshot.invitation.is_some() || Instant::now() >= deadline {
                break snapshot;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        assert!(snapshot.running, "{}", snapshot.status);
        assert!(
            snapshot.invitation.is_some(),
            "listener did not publish an invitation"
        );
        lab.control(Some(35), None).unwrap();
        lab.control(None, Some(true)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let state = lab.refresh();
            if state.volume == Some(35) && state.muted == Some(true) {
                break;
            }
            assert!(
                state.running && Instant::now() < deadline,
                "control was not acknowledged"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(lab.start("127.0.0.1", None).is_err());
        assert!(!lab.stop().running);
        assert!(lab.child.is_none());

        lab.start("127.0.0.1", Some("unavailable-output-id"))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while lab.refresh().running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(!lab.refresh().running, "unavailable device must fail");
    }
}
