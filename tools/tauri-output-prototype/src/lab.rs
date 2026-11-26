use serde::Serialize;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::Ipv4Addr;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::time::{Duration, Instant};

const MAX_LAB_EVENTS: usize = 32;
const MAX_LAB_EVENT_CHARS: usize = 256;
const SIGNAL_STALE_AFTER: Duration = Duration::from_secs(3);
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_millis(200);
const OUTPUT_DRAIN_LIMIT: usize = 2048;
const CONTROL_PORT_IN_USE: &str = "Listener exited: control TCP port is already in use. Choose another control port and start again.";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabSnapshot {
    pub running: bool,
    pub status: String,
    pub invitation: Option<String>,
    pub events: Vec<String>,
    pub metrics: Option<LabMetrics>,
    #[serde(skip)]
    pending_media: Option<String>,
    pub signal: Option<LabSignal>,
    pub signal_stale: bool,
    pub volume: Option<u8>,
    pub muted: Option<bool>,
    pub session_active: bool,
    pub media_port: Option<u16>,
    pub control_port: Option<u16>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LabSignal {
    decoded: u32,
    peak: Option<f32>,
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
    late: Option<String>,
    resync_dropped: Option<String>,
    media: Option<String>,
    playout: Option<String>,
    adaptive: Option<AdaptiveMetrics>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct AdaptiveMetrics {
    target: u16,
    min: u16,
    max: u16,
    jitter_ms: u32,
    gated: String,
    forced: String,
    resets: String,
}

impl Default for LabSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            status: "Stopped".into(),
            invitation: None,
            events: Vec::new(),
            metrics: None,
            pending_media: None,
            signal: None,
            signal_stale: false,
            volume: None,
            muted: None,
            session_active: false,
            media_port: None,
            control_port: None,
        }
    }
}

#[derive(Default)]
pub struct Lab {
    child: Option<Child>,
    events: Option<Receiver<ObservedLine>>,
    snapshot: LabSnapshot,
    signal_received_at: Option<Instant>,
}

struct ObservedLine {
    text: String,
    received_at: Instant,
}

impl Lab {
    #[cfg(test)]
    pub fn test_output_burst(&mut self) {
        let (sender, events) = mpsc::sync_channel(128);
        let mut output = "CONTROL volume 1 muted false\n".repeat(512);
        output.push_str("CONTROL volume 42 muted false\n");
        read_output(std::io::Cursor::new(output.into_bytes()), sender);
        self.events = Some(events);
    }

    #[cfg(test)]
    pub fn test_volume(&self) -> Option<u8> {
        self.snapshot.volume
    }

    #[cfg(test)]
    pub fn test_take_stdin(&mut self) -> std::process::ChildStdin {
        self.child.as_mut().unwrap().stdin.take().unwrap()
    }

    #[cfg(test)]
    pub fn simulate_running(&mut self) {
        self.snapshot.running = true;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start(
        &mut self,
        host: &str,
        output_id: Option<&str>,
        startup_buffer_ms: u16,
        adaptive: bool,
        media_port: u16,
        control_port: u16,
        initial_volume: u8,
    ) -> Result<LabSnapshot, String> {
        self.refresh();
        if self.child.is_some() {
            return Err("The listener is already running.".into());
        }
        if startup_buffer_ms > 120 || !startup_buffer_ms.is_multiple_of(20) {
            return Err("Startup buffer must be 0 to 120 ms in 20 ms steps.".into());
        }
        if media_port == 0 {
            return Err("Receiver UDP port must be between 1 and 65535.".into());
        }
        if control_port == 0 {
            return Err("Control TCP port must be between 1 and 65535.".into());
        }
        if initial_volume > 100 {
            return Err("Initial volume must be between 0 and 100.".into());
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
                "--signal-status",
                "--control",
                "--initial-volume",
                &initial_volume.to_string(),
                "--bind",
                "0.0.0.0",
                "--host",
                &host.to_string(),
                "--startup-buffer-ms",
                &startup_buffer_ms.to_string(),
                "--media-port",
                &media_port.to_string(),
                "--control-port",
                &control_port.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if adaptive {
            command.arg("--adaptive");
        }
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
        self.signal_received_at = None;
        self.snapshot = LabSnapshot {
            running: true,
            status: "Starting listener…".into(),
            invitation: None,
            events: Vec::new(),
            metrics: None,
            pending_media: None,
            signal: None,
            signal_stale: false,
            volume: None,
            muted: None,
            session_active: false,
            media_port: Some(media_port),
            control_port: Some(control_port),
        };
        Ok(self.snapshot.clone())
    }

    pub fn refresh(&mut self) -> LabSnapshot {
        self.refresh_at(Instant::now())
    }

    fn refresh_at(&mut self, now: Instant) -> LabSnapshot {
        self.drain_events(false);
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(_)) => {
                    self.drain_events(true);
                    self.snapshot.running = false;
                    self.snapshot.invitation = None;
                    self.snapshot.volume = None;
                    self.snapshot.muted = None;
                    self.snapshot.session_active = false;
                    self.snapshot.media_port = None;
                    self.snapshot.control_port = None;
                    self.snapshot.signal = None;
                    self.signal_received_at = None;
                    if self.snapshot.status != CONTROL_PORT_IN_USE {
                        self.snapshot.status = "Listener exited. Rebuild it and check the selected output device and whether the control TCP port is in use.".into();
                    }
                    self.child = None;
                    self.events = None;
                }
                Err(_) => self.snapshot.status = "Could not check the listener process.".into(),
                Ok(None) => {}
            }
        }
        self.snapshot.signal_stale = self.snapshot.signal.is_some()
            && self.signal_received_at.is_none_or(|received| {
                now.saturating_duration_since(received) >= SIGNAL_STALE_AFTER
            });
        self.snapshot.clone()
    }

    pub fn stop(&mut self) -> LabSnapshot {
        self.refresh();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.drain_events(true);
        self.events = None;
        self.signal_received_at = None;
        self.snapshot.running = false;
        self.snapshot.status = "Stopped".into();
        self.snapshot.invitation = None;
        self.snapshot.volume = None;
        self.snapshot.muted = None;
        self.snapshot.session_active = false;
        self.snapshot.media_port = None;
        self.snapshot.control_port = None;
        self.snapshot.signal = None;
        self.snapshot.signal_stale = false;
        self.snapshot.clone()
    }

    fn drain_events(&mut self, wait_for_readers: bool) {
        let Some(events) = &self.events else { return };
        let deadline = Instant::now() + OUTPUT_DRAIN_TIMEOUT;
        let limit = if wait_for_readers {
            OUTPUT_DRAIN_LIMIT
        } else {
            128
        };
        for _ in 0..limit {
            let line = if wait_for_readers {
                events
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .ok()
            } else {
                events.try_recv().ok()
            };
            let Some(line) = line else { break };
            record_event(&mut self.snapshot, &line.text);
            let valid_signal = self.snapshot.session_active && parse_signal(&line.text).is_some();
            update_snapshot(&mut self.snapshot, &line.text);
            if valid_signal {
                self.signal_received_at = Some(line.received_at);
            } else if self.snapshot.signal.is_none() {
                self.signal_received_at = None;
            }
        }
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

fn read_output(output: impl Read + Send + 'static, sender: SyncSender<ObservedLine>) {
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
                    // Apply bounded backpressure instead of silently dropping a
                    // state transition when the UI is between snapshot polls.
                    if sender
                        .send(ObservedLine {
                            text: text.trim_end_matches(['\r', '\n']).to_owned(),
                            received_at: Instant::now(),
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                line.clear();
                oversized = false;
            }
            reader.consume(consumed);
        }
    });
}

fn update_snapshot(snapshot: &mut LabSnapshot, line: &str) {
    if line == "STARTUP control-port-in-use" {
        snapshot.status = CONTROL_PORT_IN_USE.into();
    } else if let ["CONTROL", "volume", volume, "muted", muted] =
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
        if !matches!(
            snapshot.status.as_str(),
            "Session ended; waiting for Android" | "Session failed; waiting for Android"
        ) {
            snapshot.status = "Waiting for Android".into();
        }
        snapshot.session_active = false;
        snapshot.signal = None;
    } else if session_event(line, "RESULT client connected") {
        snapshot.status = "Android connecting".into();
        snapshot.metrics = None;
        snapshot.pending_media = None;
        snapshot.signal = None;
        snapshot.session_active = false;
    } else if line == "RESULT pairing verified" {
        snapshot.status = "Paired; negotiating session".into();
    } else if session_event(line, "RESULT receiver started") {
        snapshot.status = "Session active".into();
        snapshot.metrics = None;
        snapshot.pending_media = None;
        snapshot.signal = None;
        snapshot.session_active = true;
    } else if snapshot.session_active
        && let Some(signal) = parse_signal(line)
    {
        snapshot.signal = Some(signal);
    } else if snapshot.session_active
        && let Some(state @ ("waiting" | "receiving" | "interrupted")) = line.strip_prefix("MEDIA ")
    {
        snapshot.pending_media = Some(state.into());
        if let Some(metrics) = &mut snapshot.metrics {
            metrics.media = snapshot.pending_media.clone();
        }
    } else if snapshot.session_active
        && let Some(state @ ("active" | "recovering" | "software-check")) =
            line.strip_prefix("OUTPUT ")
        && let Some(metrics) = &mut snapshot.metrics
    {
        metrics.output = state.into();
    } else if snapshot.session_active
        && let Some(mut metrics) = parse_metrics(line)
    {
        metrics.media = snapshot.pending_media.clone();
        snapshot.metrics = Some(metrics);
    } else if line == "RESULT session complete" {
        snapshot.status = "Session ended; waiting for Android".into();
        snapshot.session_active = false;
        snapshot.signal = None;
    } else if line == "RESULT session failed" {
        snapshot.status = "Session failed; waiting for Android".into();
        snapshot.session_active = false;
        snapshot.signal = None;
    }
}

fn session_event(line: &str, event: &str) -> bool {
    line == event
        || line
            .strip_prefix(event)
            .and_then(|suffix| suffix.strip_prefix(" session "))
            .and_then(|suffix| suffix.split_whitespace().next())
            .and_then(|number| number.parse::<u64>().ok())
            .is_some_and(|number| number > 0)
}

fn parse_signal(line: &str) -> Option<LabSignal> {
    let parts: Vec<_> = line.split_whitespace().collect();
    let ["SIGNAL", "decoded", decoded, "peak", peak] = parts.as_slice() else {
        return None;
    };
    let decoded = decoded.parse::<u32>().ok()?;
    let peak = if *peak == "none" {
        None
    } else {
        let peak = peak.parse::<f32>().ok()?;
        if !peak.is_finite() || peak < 0.0 {
            return None;
        }
        Some(peak)
    };
    if (decoded == 0) != peak.is_none() {
        return None;
    }
    Some(LabSignal { decoded, peak })
}

fn record_event(snapshot: &mut LabSnapshot, line: &str) {
    let is_event = matches!(line, "READY")
        || line.starts_with("RESULT ")
        || line.starts_with("MEDIA ")
        || line.starts_with("OUTPUT ");
    if !is_event {
        return;
    }
    if snapshot.events.len() == MAX_LAB_EVENTS {
        snapshot.events.remove(0);
    }
    let safe_line = if line.starts_with("RESULT client connected ") {
        "RESULT client connected"
    } else {
        line
    };
    snapshot.events.push(bound_event(safe_line));
}

fn bound_event(line: &str) -> String {
    let mut chars = line.chars();
    let bounded: String = chars.by_ref().take(MAX_LAB_EVENT_CHARS).collect();
    if chars.next().is_some() {
        format!("{bounded}...")
    } else {
        bounded
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
        rest @ ..,
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
        || !matches!(
            *output,
            "active" | "recovering" | "software-check" | "stopped"
        )
    {
        return None;
    }
    let (late, resync_dropped, rest) = match rest {
        ["late", late, "resync_dropped", dropped, rest @ ..] => {
            late.parse::<u64>().ok()?;
            dropped.parse::<u64>().ok()?;
            (Some((*late).into()), Some((*dropped).into()), rest)
        }
        _ => (None, None, rest),
    };
    let (playout, adaptive) = match rest {
        [] => (None, None),
        ["playout", "fixed"] => (Some("fixed".into()), None),
        [
            "playout",
            "adaptive",
            "target",
            target,
            "min",
            min,
            "max",
            max,
            "jitter_ms",
            jitter_ms,
            "gated",
            gated,
            "forced",
            forced,
            "resets",
            resets,
        ] => {
            let (target, min, max) = (
                target.parse::<u16>().ok()?,
                min.parse::<u16>().ok()?,
                max.parse::<u16>().ok()?,
            );
            if !((target == 0 && min == 0 && max == 0)
                || (min > 0 && min <= target && target <= max && max <= 8))
            {
                return None;
            }
            let jitter_ms = jitter_ms.parse::<u32>().ok()?;
            for value in [gated, forced, resets] {
                value.parse::<u64>().ok()?;
            }
            (
                Some("adaptive".into()),
                Some(AdaptiveMetrics {
                    target,
                    min,
                    max,
                    jitter_ms,
                    gated: (*gated).into(),
                    forced: (*forced).into(),
                    resets: (*resets).into(),
                }),
            )
        }
        _ => return None,
    };
    Some(LabMetrics {
        packets: (*packets).into(),
        rejected: (*rejected).into(),
        decoded: (*decoded).into(),
        peak: (*peak).into(),
        plc: (*plc).into(),
        decode_errors: (*decode_errors).into(),
        buffered: (*buffered).into(),
        output: (*output).into(),
        late,
        resync_dropped,
        media: None,
        playout,
        adaptive,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_freshness_uses_read_time_and_resets_with_session() {
        let (sender, events) = mpsc::sync_channel(8);
        let start = Instant::now();
        let send = |text: &str, received_at| {
            sender
                .send(ObservedLine {
                    text: text.into(),
                    received_at,
                })
                .unwrap();
        };
        let mut lab = Lab::default();
        lab.events = Some(events);
        send("RESULT receiver started", start);
        send("SIGNAL decoded 50 peak 0.1", start);
        assert!(
            !lab.refresh_at(start + Duration::from_millis(2999))
                .signal_stale
        );
        send("SIGNAL decoded 1 peak NaN", start + Duration::from_secs(3));
        assert!(lab.refresh_at(start + Duration::from_secs(3)).signal_stale);
        send("SIGNAL decoded 0 peak none", start + Duration::from_secs(4));
        assert!(!lab.refresh_at(start + Duration::from_secs(4)).signal_stale);
        send("RESULT session complete", start + Duration::from_secs(5));
        let ended = lab.refresh_at(start + Duration::from_secs(5));
        assert!(ended.signal.is_none());
        assert!(!ended.signal_stale);
        send("RESULT receiver started", start + Duration::from_secs(6));
        let next = lab.refresh_at(start + Duration::from_secs(6));
        assert!(next.signal.is_none());
        assert!(!next.signal_stale);
        send("SIGNAL decoded 50 peak 0.1", start + Duration::from_secs(6));
        assert!(lab.refresh_at(start + Duration::from_secs(10)).signal_stale);
    }

    #[test]
    fn signal_observations_follow_session_lifecycle() {
        let mut snapshot = LabSnapshot::default();
        update_snapshot(&mut snapshot, "SIGNAL decoded 50 peak 0.1");
        assert_eq!(snapshot.signal, None);
        update_snapshot(&mut snapshot, "RESULT receiver started");
        for (line, expected) in [
            (
                "SIGNAL decoded 50 peak 0.1",
                Some(LabSignal {
                    decoded: 50,
                    peak: Some(0.1),
                }),
            ),
            (
                "SIGNAL decoded 50 peak 0.000000",
                Some(LabSignal {
                    decoded: 50,
                    peak: Some(0.0),
                }),
            ),
            (
                "SIGNAL decoded 0 peak none",
                Some(LabSignal {
                    decoded: 0,
                    peak: None,
                }),
            ),
        ] {
            update_snapshot(&mut snapshot, line);
            assert_eq!(snapshot.signal, expected);
            record_event(&mut snapshot, line);
        }
        assert!(snapshot.events.is_empty());
        for invalid in [
            "SIGNAL decoded 0 peak 0",
            "SIGNAL decoded 1 peak none",
            "SIGNAL decoded 1 peak NaN",
            "SIGNAL decoded 1 peak -1",
            "SIGNAL decoded 1 peak 0 extra",
        ] {
            update_snapshot(&mut snapshot, invalid);
            assert_eq!(
                snapshot.signal,
                Some(LabSignal {
                    decoded: 0,
                    peak: None
                })
            );
        }
        update_snapshot(&mut snapshot, "RESULT session complete");
        assert_eq!(snapshot.signal, None);
        update_snapshot(&mut snapshot, "RESULT receiver started");
        assert_eq!(snapshot.signal, None);
        update_snapshot(&mut snapshot, "SIGNAL decoded 1 peak 0.1");
        update_snapshot(&mut snapshot, "RESULT session failed");
        assert_eq!(snapshot.signal, None);
    }

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
    fn numbered_listener_events_enable_metrics_and_hide_endpoints() {
        let mut snapshot = LabSnapshot::default();
        let connected =
            "RESULT client connected session 1 local=127.0.0.1:47610 peer=127.0.0.1:12345";
        record_event(&mut snapshot, connected);
        update_snapshot(&mut snapshot, connected);
        assert_eq!(snapshot.status, "Android connecting");
        assert_eq!(snapshot.events, ["RESULT client connected"]);
        update_snapshot(&mut snapshot, "RESULT pairing verified");
        update_snapshot(&mut snapshot, "RESULT receiver started session 1");
        assert!(snapshot.session_active);
        update_snapshot(
            &mut snapshot,
            "STATUS packets 9 rejected 0 decoded 8 peak 0.100 plc 1 decode_errors 0 buffered 0 output active",
        );
        update_snapshot(&mut snapshot, "SIGNAL decoded 8 peak 0.100");
        assert_eq!(snapshot.metrics.as_ref().unwrap().decoded, "8");
        assert_eq!(snapshot.signal.as_ref().unwrap().decoded, 8);
        update_snapshot(&mut snapshot, "RESULT session complete");
        assert!(!snapshot.session_active);
        update_snapshot(
            &mut snapshot,
            "RESULT client connected session 2 local=127.0.0.1:47610 peer=127.0.0.1:12346",
        );
        assert_eq!(snapshot.status, "Android connecting");
        assert!(snapshot.metrics.is_none());
        update_snapshot(&mut snapshot, "RESULT receiver started session invalid");
        assert!(!snapshot.session_active);
    }

    #[test]
    fn rejects_invalid_host_before_launch() {
        let mut lab = Lab::default();
        for host in ["invalid", "0.0.0.0", "224.0.0.1", "255.255.255.255"] {
            assert!(lab.start(host, None, 60, false, 39093, 47610, 100).is_err());
        }
        for value in [1, 19, 30, 121, 140] {
            assert!(
                lab.start("127.0.0.1", None, value, false, 39093, 47610, 100)
                    .is_err()
            );
        }
        assert!(
            lab.start("127.0.0.1", None, 60, false, 0, 47610, 100)
                .is_err()
        );
        assert!(
            lab.start("127.0.0.1", None, 60, false, 39093, 0, 100)
                .is_err()
        );
        assert!(
            lab.start("127.0.0.1", None, 60, false, 39093, 47610, 101)
                .is_err()
        );
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
        assert_eq!(snapshot.metrics, Some(metrics.clone()));
        update_snapshot(&mut snapshot, "READY");
        assert_eq!(snapshot.metrics, Some(metrics.clone()));
        update_snapshot(&mut snapshot, status);
        assert_eq!(snapshot.metrics, Some(metrics));
        assert!(parse_metrics(&status.replace("0.250", "NaN")).is_none());
        assert!(parse_metrics(&status.replace("recovering", "unknown")).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn control_port_conflict_survives_listener_exit() {
        let mut child = Command::new("cmd.exe")
            .args(["/C", "exit", "1"])
            .spawn()
            .unwrap();
        child.wait().unwrap();
        let (sender, events) = mpsc::sync_channel(1);
        sender
            .send(ObservedLine {
                text: "STARTUP control-port-in-use".into(),
                received_at: Instant::now(),
            })
            .unwrap();
        drop(sender);
        let mut lab = Lab {
            child: Some(child),
            events: Some(events),
            snapshot: LabSnapshot {
                running: true,
                control_port: Some(47610),
                ..LabSnapshot::default()
            },
            signal_received_at: None,
        };
        let snapshot = lab.refresh();
        assert!(!snapshot.running);
        assert_eq!(snapshot.status, CONTROL_PORT_IN_USE);
        assert!(snapshot.control_port.is_none());
    }

    #[cfg(windows)]
    #[test]
    fn exited_listener_collects_output_delivered_after_process_exit() {
        let mut child = Command::new("cmd.exe")
            .args(["/C", "exit", "1"])
            .spawn()
            .unwrap();
        child.wait().unwrap();
        let (sender, events) = mpsc::sync_channel(4);
        let reader = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            for text in [
                "RESULT receiver started",
                "STATUS packets 9 rejected 0 decoded 8 peak 0.100 plc 1 decode_errors 0 buffered 0 output active",
                "RESULT session complete",
            ] {
                sender
                    .send(ObservedLine {
                        text: text.into(),
                        received_at: Instant::now(),
                    })
                    .unwrap();
            }
        });
        let mut lab = Lab {
            child: Some(child),
            events: Some(events),
            snapshot: LabSnapshot {
                running: true,
                ..LabSnapshot::default()
            },
            signal_received_at: None,
        };
        let snapshot = lab.refresh();
        reader.join().unwrap();
        assert!(!snapshot.running && !snapshot.session_active);
        assert_eq!(snapshot.metrics.unwrap().decoded, "8");
        assert!(
            snapshot
                .events
                .iter()
                .any(|event| event == "RESULT session complete")
        );
        assert!(snapshot.status.contains("Listener exited"));
    }

    #[cfg(windows)]
    #[test]
    fn unexpected_listener_exit_retains_last_observations_after_explicit_stop() {
        let child = Command::new("cmd.exe")
            .args(["/C", "exit", "1"])
            .spawn()
            .unwrap();
        let (sender, events) = mpsc::sync_channel(4);
        for line in [
            "RESULT receiver started",
            "STATUS packets 120 rejected 3 decoded 115 peak 0.250 plc 2 decode_errors 1 buffered 4 output active",
        ] {
            sender
                .try_send(ObservedLine {
                    text: line.into(),
                    received_at: Instant::now(),
                })
                .unwrap();
        }
        let mut lab = Lab {
            child: Some(child),
            events: Some(events),
            snapshot: LabSnapshot {
                running: true,
                invitation: Some("01ab".into()),
                volume: Some(50),
                muted: Some(false),
                ..LabSnapshot::default()
            },
            signal_received_at: None,
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let snapshot = loop {
            let snapshot = lab.refresh();
            if !snapshot.running {
                break snapshot;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "listener did not exit"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(snapshot.metrics.unwrap().decoded, "115");
        assert!(!snapshot.session_active);
        assert!(snapshot.invitation.is_none());
        assert!(snapshot.volume.is_none());
        assert!(snapshot.muted.is_none());
        assert!(snapshot.status.contains("Listener exited"));
        assert_eq!(lab.stop().metrics.unwrap().decoded, "115");
    }

    #[test]
    fn stop_retains_pending_observations_and_clears_live_state() {
        let (sender, events) = mpsc::sync_channel(8);
        for line in [
            "RESULT receiver started",
            "STATUS packets 120 rejected 3 decoded 115 peak 0.250 plc 2 decode_errors 1 buffered 4 output active",
            "SIGNAL decoded 50 peak 0.1",
        ] {
            sender
                .send(ObservedLine {
                    text: line.into(),
                    received_at: Instant::now(),
                })
                .unwrap();
        }
        let mut lab = Lab {
            child: None,
            events: Some(events),
            snapshot: LabSnapshot {
                running: true,
                invitation: Some("01ab".into()),
                volume: Some(50),
                muted: Some(false),
                ..LabSnapshot::default()
            },
            signal_received_at: None,
        };
        let stopped = lab.stop();
        assert_eq!(stopped.status, "Stopped");
        assert!(!stopped.running && !stopped.session_active);
        assert!(
            stopped.invitation.is_none() && stopped.volume.is_none() && stopped.muted.is_none()
        );
        assert!(stopped.signal.is_none() && !stopped.signal_stale);
        assert_eq!(stopped.metrics.unwrap().decoded, "115");
        assert_eq!(stopped.events, ["RESULT receiver started"]);
        assert_eq!(lab.stop().metrics.unwrap().decoded, "115");
    }

    #[test]
    fn parses_optional_late_and_resync_counts_without_losing_large_integers() {
        let prefix = "STATUS packets 120 rejected 3 decoded 115 peak 0.250 plc 2 decode_errors 1 buffered 4 output active";
        let old = parse_metrics(&format!("{prefix} playout fixed")).unwrap();
        assert_eq!((old.late, old.resync_dropped), (None, None));
        assert_eq!(old.media, None);
        let extended = format!("{prefix} late 9007199254740993 resync_dropped 7 playout fixed");
        let parsed = parse_metrics(&extended).unwrap();
        assert_eq!(parsed.late.as_deref(), Some("9007199254740993"));
        assert_eq!(parsed.resync_dropped.as_deref(), Some("7"));
        assert_eq!(parsed.playout.as_deref(), Some("fixed"));
        assert!(
            parse_metrics(&extended.replace("resync_dropped 7", "resync_dropped invalid"))
                .is_none()
        );
        assert!(parse_metrics(&format!("{prefix} late 1 playout fixed")).is_none());
        assert!(parse_metrics(&format!("{prefix} late 2 resync_dropped 3 playout adaptive target 3 min 2 max 4 jitter_ms 17 gated 7 forced 2 resets 1")).is_some());
    }

    #[test]
    fn media_observation_follows_status_and_stays_with_last_session() {
        let mut snapshot = LabSnapshot::default();
        let status = "STATUS packets 1 rejected 0 decoded 1 peak 0.100 plc 0 decode_errors 0 buffered 0 output active playout fixed";
        update_snapshot(&mut snapshot, "RESULT receiver started");
        update_snapshot(&mut snapshot, status);
        assert_eq!(snapshot.metrics.as_ref().unwrap().media, None);
        for state in ["waiting", "receiving", "interrupted"] {
            update_snapshot(&mut snapshot, &format!("MEDIA {state}"));
            update_snapshot(&mut snapshot, status);
            assert_eq!(
                snapshot.metrics.as_ref().unwrap().media.as_deref(),
                Some(state)
            );
        }
        update_snapshot(&mut snapshot, "MEDIA invalid");
        update_snapshot(&mut snapshot, "RESULT session complete");
        update_snapshot(&mut snapshot, "MEDIA receiving");
        assert_eq!(
            snapshot.metrics.as_ref().unwrap().media.as_deref(),
            Some("interrupted")
        );
        update_snapshot(&mut snapshot, "RESULT client connected");
        assert!(snapshot.metrics.is_none());
    }

    #[test]
    fn media_before_first_metrics_is_retained_only_for_its_session() {
        let mut snapshot = LabSnapshot::default();
        let status = "STATUS packets 1 rejected 0 decoded 1 peak 0.100 plc 0 decode_errors 0 buffered 0 output active playout fixed";
        update_snapshot(&mut snapshot, "RESULT receiver started session 1");
        update_snapshot(&mut snapshot, "MEDIA waiting");
        update_snapshot(&mut snapshot, "MEDIA receiving");
        assert!(snapshot.metrics.is_none());
        update_snapshot(&mut snapshot, status);
        assert_eq!(
            snapshot.metrics.as_ref().unwrap().media.as_deref(),
            Some("receiving")
        );
        update_snapshot(&mut snapshot, "RESULT session complete");
        update_snapshot(&mut snapshot, "RESULT client connected session 2");
        update_snapshot(&mut snapshot, "RESULT receiver started session 2");
        update_snapshot(&mut snapshot, status);
        assert_eq!(snapshot.metrics.as_ref().unwrap().media, None);
    }

    #[test]
    fn output_observation_updates_promptly_without_accepting_invalid_states() {
        let mut snapshot = LabSnapshot::default();
        let status = "STATUS packets 1 rejected 0 decoded 1 peak 0.100 plc 0 decode_errors 0 buffered 0 output active playout fixed";
        update_snapshot(&mut snapshot, "RESULT receiver started");
        update_snapshot(&mut snapshot, status);
        for state in ["recovering", "active", "software-check"] {
            update_snapshot(&mut snapshot, &format!("OUTPUT {state}"));
            assert_eq!(snapshot.metrics.as_ref().unwrap().output, state);
        }
        let before = snapshot.metrics.clone();
        update_snapshot(&mut snapshot, "OUTPUT failed");
        assert_eq!(snapshot.metrics, before);
        update_snapshot(&mut snapshot, "RESULT session complete");
        update_snapshot(&mut snapshot, "OUTPUT recovering");
        assert_eq!(snapshot.metrics.as_ref().unwrap().output, "software-check");
    }

    #[test]
    fn adaptive_observations_follow_epoch_and_session_lifecycle() {
        let prefix = "STATUS packets 120 rejected 3 decoded 115 peak 0.250 plc 2 decode_errors 1 buffered 4 output active";
        let mut snapshot = LabSnapshot::default();
        update_snapshot(&mut snapshot, "RESULT receiver started");
        update_snapshot(
            &mut snapshot,
            &format!(
                "{prefix} playout adaptive target 3 min 2 max 4 jitter_ms 17 gated 9007199254740993 forced 2 resets 1"
            ),
        );
        let metrics = snapshot.metrics.clone().unwrap();
        assert_eq!(metrics.playout.as_deref(), Some("adaptive"));
        let adaptive = metrics.adaptive.unwrap();
        assert_eq!(
            (
                adaptive.target,
                adaptive.min,
                adaptive.max,
                adaptive.jitter_ms
            ),
            (3, 2, 4, 17)
        );
        assert_eq!(adaptive.gated, "9007199254740993");
        assert_eq!(adaptive.resets, "1");
        let before = snapshot.metrics.clone();
        update_snapshot(
            &mut snapshot,
            &format!(
                "{prefix} playout adaptive target 9 min 2 max 4 jitter_ms 17 gated 3 forced 2 resets 1"
            ),
        );
        assert_eq!(snapshot.metrics, before);
        update_snapshot(
            &mut snapshot,
            &format!(
                "{prefix} playout adaptive target 0 min 0 max 0 jitter_ms 0 gated 0 forced 0 resets 2"
            ),
        );
        let reset = snapshot
            .metrics
            .as_ref()
            .unwrap()
            .adaptive
            .as_ref()
            .unwrap();
        assert_eq!(
            (reset.target, reset.gated.as_str(), reset.resets.as_str()),
            (0, "0", "2")
        );
        update_snapshot(&mut snapshot, "RESULT session complete");
        update_snapshot(&mut snapshot, "READY");
        assert_eq!(
            snapshot
                .metrics
                .as_ref()
                .unwrap()
                .adaptive
                .as_ref()
                .unwrap()
                .resets,
            "2"
        );
        update_snapshot(&mut snapshot, "RESULT client connected");
        assert!(snapshot.metrics.is_none());
        let fixed = parse_metrics(&format!("{prefix} playout fixed")).unwrap();
        assert_eq!(fixed.playout.as_deref(), Some("fixed"));
        assert!(fixed.adaptive.is_none());
        assert!(parse_metrics(prefix).unwrap().playout.is_none());
        assert!(parse_metrics(&format!("{prefix} playout adaptive target 3 min 2 max 4 jitter_ms 17 gated invalid forced 2 resets 1")).is_none());
    }

    #[test]
    fn session_results_retain_observations_until_next_client() {
        let status = "STATUS packets 120 rejected 3 decoded 115 peak 0.250 plc 2 decode_errors 1 buffered 4 output active";
        for (result, expected) in [
            (
                "RESULT session failed",
                "Session failed; waiting for Android",
            ),
            (
                "RESULT session complete",
                "Session ended; waiting for Android",
            ),
        ] {
            let mut snapshot = LabSnapshot::default();
            update_snapshot(&mut snapshot, "RESULT receiver started");
            update_snapshot(&mut snapshot, status);
            assert!(snapshot.metrics.is_some());
            update_snapshot(&mut snapshot, result);
            assert!(!snapshot.session_active);
            assert!(snapshot.metrics.is_some());
            update_snapshot(&mut snapshot, "READY");
            assert_eq!(snapshot.status, expected);
            assert!(snapshot.metrics.is_some());
            update_snapshot(&mut snapshot, "RESULT client connected");
            assert!(snapshot.metrics.is_none());
            update_snapshot(&mut snapshot, "RESULT pairing verified");
            update_snapshot(&mut snapshot, "Session ended: delayed stderr");
            assert_eq!(snapshot.status, "Paired; negotiating session");
            update_snapshot(&mut snapshot, "RESULT receiver started");
            update_snapshot(&mut snapshot, status);
            assert!(snapshot.metrics.is_some());
        }
    }

    #[test]
    fn final_status_is_retained_with_the_post_drain_counts() {
        let mut snapshot = LabSnapshot::default();
        update_snapshot(&mut snapshot, "RESULT receiver started");
        update_snapshot(
            &mut snapshot,
            "STATUS packets 10 rejected 0 decoded 9 peak 0.125 plc 1 decode_errors 0 buffered 2 output active",
        );
        update_snapshot(
            &mut snapshot,
            "STATUS packets 12 rejected 0 decoded 12 peak 0.250 plc 1 decode_errors 0 buffered 0 output stopped",
        );
        update_snapshot(&mut snapshot, "RESULT session complete");
        update_snapshot(&mut snapshot, "READY");
        let metrics = snapshot.metrics.unwrap();
        assert_eq!(metrics.decoded, "12");
        assert_eq!(metrics.output, "stopped");
        assert!(!snapshot.session_active);
    }

    #[test]
    fn read_output_preserves_events_when_snapshot_polling_is_delayed() {
        use std::io::Cursor;
        use std::time::Duration;

        let (sender, events) = mpsc::sync_channel(1);
        read_output(
            Cursor::new("READY\nRESULT client connected\nRESULT pairing verified\n"),
            sender,
        );

        for expected in [
            "READY",
            "RESULT client connected",
            "RESULT pairing verified",
        ] {
            assert_eq!(
                events.recv_timeout(Duration::from_secs(1)).unwrap().text,
                expected
            );
        }
    }

    #[test]
    fn refresh_retains_a_bounded_ordered_lifecycle_event_log() {
        let (sender, events) = mpsc::sync_channel(8);
        for line in [
            "READY",
            "RESULT client connected",
            "MEDIA receiving",
            "OUTPUT recovering",
        ] {
            sender
                .send(ObservedLine {
                    text: line.to_owned(),
                    received_at: Instant::now(),
                })
                .unwrap();
        }
        drop(sender);

        let mut lab = Lab::default();
        lab.events = Some(events);
        let snapshot = lab.refresh();

        assert_eq!(
            snapshot.events,
            vec![
                "READY",
                "RESULT client connected",
                "MEDIA receiving",
                "OUTPUT recovering",
            ]
        );
        assert_eq!(snapshot.status, "Android connecting");

        for index in 0..(MAX_LAB_EVENTS + 4) {
            record_event(&mut lab.snapshot, &format!("RESULT event {index}"));
        }
        assert_eq!(lab.snapshot.events.len(), MAX_LAB_EVENTS);
        assert_eq!(
            lab.snapshot.events.first().map(String::as_str),
            Some("RESULT event 4")
        );
        assert_eq!(
            lab.snapshot.events.last().map(String::as_str),
            Some("RESULT event 35")
        );
    }

    #[test]
    fn lifecycle_events_are_bounded_without_splitting_utf8() {
        let mut snapshot = LabSnapshot::default();
        let long = format!("RESULT {}", "界".repeat(MAX_LAB_EVENT_CHARS));
        record_event(&mut snapshot, &long);
        let event = snapshot.events.first().unwrap();
        assert!(event.ends_with("..."));
        assert!(event.chars().count() <= MAX_LAB_EVENT_CHARS + 3);
        assert!(event.is_char_boundary(event.len() - 3));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires the playback lab binary and a Windows output"]
    fn real_listener_reports_control_port_conflict_and_retries() {
        let occupied = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = occupied.local_addr().unwrap().port();
        let mut lab = Lab::default();
        lab.start("127.0.0.1", None, 0, false, 39094, port, 100)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let failure = loop {
            let snapshot = lab.refresh();
            if !snapshot.running {
                break snapshot;
            }
            assert!(Instant::now() < deadline, "listener did not exit");
            std::thread::sleep(Duration::from_millis(25));
        };
        assert_eq!(failure.status, CONTROL_PORT_IN_USE);
        drop(occupied);
        lab.start("127.0.0.1", None, 0, false, 39094, port, 100)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snapshot = lab.refresh();
            assert!(snapshot.running, "{}", snapshot.status);
            if snapshot.invitation.is_some() {
                break;
            }
            assert!(Instant::now() < deadline, "listener did not become ready");
            std::thread::sleep(Duration::from_millis(25));
        }
        lab.stop();
    }

    #[test]
    #[ignore = "requires the playback lab binary, a Windows output and free lab ports"]
    fn real_listener_starts_and_stops() {
        use std::time::{Duration, Instant};
        let output = sonimesh_platform::CpalOutput::new();
        let device = output.default_device_probe().unwrap().device.id;
        let mut lab = Lab::default();
        let started = lab
            .start(
                "127.0.0.1",
                Some(device.as_str()),
                0,
                false,
                39094,
                47612,
                42,
            )
            .unwrap();
        assert_eq!(started.media_port, Some(39094));
        assert_eq!(started.control_port, Some(47612));
        let deadline = Instant::now() + Duration::from_secs(10);
        let snapshot = loop {
            let snapshot = lab.refresh();
            if !snapshot.running
                || (snapshot.invitation.is_some() && snapshot.volume.is_some())
                || Instant::now() >= deadline
            {
                break snapshot;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        assert!(snapshot.running, "{}", snapshot.status);
        assert_eq!(snapshot.media_port, Some(39094));
        assert_eq!(snapshot.control_port, Some(47612));
        assert_eq!(snapshot.volume, Some(42));
        assert_eq!(snapshot.muted, Some(false));
        assert!(
            snapshot.invitation.is_some(),
            "listener did not publish an invitation"
        );
        drop(std::net::TcpStream::connect("127.0.0.1:47612").unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = lab.refresh();
            if state.status == "Session failed; waiting for Android" {
                assert!(
                    state
                        .events
                        .iter()
                        .any(|event| event == "RESULT client connected")
                );
                assert!(state.events.iter().all(|event| !event.contains("peer=")));
                break;
            }
            assert!(
                state.running && Instant::now() < deadline,
                "{}",
                state.status
            );
            std::thread::sleep(Duration::from_millis(25));
        }
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
        assert!(
            lab.start("127.0.0.1", None, 60, false, 39093, 47610, 100)
                .is_err()
        );
        let stopped = lab.stop();
        assert!(!stopped.running);
        assert_eq!(stopped.media_port, None);
        assert!(stopped.invitation.is_none());
        assert!(
            stopped
                .events
                .iter()
                .any(|event| event == "RESULT session failed")
        );
        assert!(lab.child.is_none());

        let restarted = lab
            .start(
                "127.0.0.1",
                Some("unavailable-output-id"),
                60,
                false,
                39093,
                47610,
                100,
            )
            .unwrap();
        assert!(restarted.events.is_empty());
        assert!(restarted.metrics.is_none());
        assert_eq!(restarted.media_port, Some(39093));
        let deadline = Instant::now() + Duration::from_secs(10);
        while lab.refresh().running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(!lab.refresh().running, "unavailable device must fail");
        assert_eq!(lab.refresh().media_port, None);
    }

    #[test]
    #[ignore = "requires the playback lab binary, Windows output and an installed API 37 emulator test"]
    fn emulator_capture_updates_real_listener_snapshot() {
        let mut lab = Lab::default();
        lab.start("10.0.2.2", None, 60, false, 39094, 47610, 100)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let invitation = loop {
            let snapshot = lab.refresh();
            if let Some(invitation) = snapshot.invitation {
                break invitation;
            }
            assert!(
                snapshot.running && Instant::now() < deadline,
                "{}",
                snapshot.status
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        let direct_invitation = format!("SONIMESH-LAB1:39094:{invitation}");
        let mut instrumentation = Command::new("adb")
            .args([
                "-s",
                "emulator-5556",
                "shell",
                "am",
                "instrument",
                "-w",
                "-e",
                "pairingInvitationHex",
                &direct_invitation,
                "-e",
                "mediaHost",
                "10.0.2.2",
                "-e",
                "expectedMediaPort",
                "39094",
                "-e",
                "bitrateKbps",
                "96",
                "-e",
                "class",
                "com.sonimesh.opusspike.PairedCaptureTest#captures_when_next_invitation_changes_during_consent",
                "com.sonimesh.opusspike.test/androidx.test.runner.AndroidJUnitRunner",
            ])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(50);
        let mut observed_signal = false;
        let mut last_observation = String::new();
        while Instant::now() < deadline {
            let snapshot = lab.refresh();
            last_observation = format!(
                "status={} active={} metrics={:?} signal={:?} events={:?}",
                snapshot.status,
                snapshot.session_active,
                snapshot.metrics,
                snapshot.signal,
                snapshot.events
            );
            if snapshot.session_active
                && snapshot
                    .signal
                    .as_ref()
                    .is_some_and(|signal| signal.decoded > 0)
            {
                observed_signal = true;
            }
            if instrumentation.try_wait().unwrap().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        if instrumentation.try_wait().unwrap().is_none() {
            instrumentation.kill().unwrap();
        }
        let output = instrumentation.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            text.contains("OK (1 test)"),
            "instrumentation failed: {text}"
        );
        assert!(
            observed_signal,
            "listener snapshot did not report an active decoded signal: {last_observation}"
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        let final_snapshot = loop {
            let snapshot = lab.refresh();
            if !snapshot.session_active || Instant::now() >= deadline {
                break snapshot;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(
            final_snapshot.metrics.as_ref().is_some_and(|metrics| {
                metrics.decoded.parse::<u64>().is_ok_and(|count| count > 0)
            }),
            "listener snapshot did not retain decoded frames: {last_observation}"
        );
        assert_eq!(
            final_snapshot
                .metrics
                .as_ref()
                .and_then(|metrics| metrics.media.as_deref()),
            Some("receiving"),
            "listener snapshot did not retain media arrival: {last_observation}"
        );
        lab.stop();
    }
}
