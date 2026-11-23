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
    pub session_active: bool,
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
    #[cfg(test)]
    pub fn simulate_running(&mut self) {
        self.snapshot.running = true;
    }

    pub fn start(
        &mut self,
        host: &str,
        output_id: Option<&str>,
        startup_buffer_ms: u16,
        adaptive: bool,
    ) -> Result<LabSnapshot, String> {
        self.refresh();
        if self.child.is_some() {
            return Err("The listener is already running.".into());
        }
        if !(20..=120).contains(&startup_buffer_ms) || !startup_buffer_ms.is_multiple_of(20) {
            return Err("Startup buffer must be 20 to 120 ms in 20 ms steps.".into());
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
                "--startup-buffer-ms",
                &startup_buffer_ms.to_string(),
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
                    // Apply bounded backpressure instead of silently dropping a
                    // state transition when the UI is between snapshot polls.
                    if sender
                        .send(text.trim_end_matches(['\r', '\n']).to_owned())
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
        if !matches!(
            snapshot.status.as_str(),
            "Session ended; waiting for Android" | "Session failed; waiting for Android"
        ) {
            snapshot.status = "Waiting for Android".into();
        }
        snapshot.session_active = false;
    } else if line == "RESULT client connected" {
        snapshot.status = "Android connecting".into();
        snapshot.metrics = None;
        snapshot.session_active = false;
    } else if line == "RESULT pairing verified" {
        snapshot.status = "Paired; negotiating session".into();
    } else if line == "RESULT receiver started" {
        snapshot.status = "Session active".into();
        snapshot.metrics = None;
        snapshot.session_active = true;
    } else if snapshot.session_active
        && let Some(state @ ("waiting" | "receiving" | "interrupted")) = line.strip_prefix("MEDIA ")
        && let Some(metrics) = &mut snapshot.metrics
    {
        metrics.media = Some(state.into());
    } else if snapshot.session_active
        && let Some(state @ ("active" | "recovering" | "software-check")) =
            line.strip_prefix("OUTPUT ")
        && let Some(metrics) = &mut snapshot.metrics
    {
        metrics.output = state.into();
    } else if snapshot.session_active
        && let Some(mut metrics) = parse_metrics(line)
    {
        metrics.media = snapshot
            .metrics
            .as_ref()
            .and_then(|previous| previous.media.clone());
        snapshot.metrics = Some(metrics);
    } else if line == "RESULT session complete" {
        snapshot.status = "Session ended; waiting for Android".into();
        snapshot.session_active = false;
    } else if line == "RESULT session failed" {
        snapshot.status = "Session failed; waiting for Android".into();
        snapshot.session_active = false;
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
            assert!(lab.start(host, None, 60, false).is_err());
        }
        for value in [0, 19, 30, 121, 140] {
            assert!(lab.start("127.0.0.1", None, value, false).is_err());
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
    fn unexpected_listener_exit_retains_last_observations_until_explicit_stop() {
        let child = Command::new("cmd.exe")
            .args(["/C", "exit", "1"])
            .spawn()
            .unwrap();
        let (sender, events) = mpsc::sync_channel(4);
        sender.try_send("RESULT receiver started".into()).unwrap();
        sender.try_send("STATUS packets 120 rejected 3 decoded 115 peak 0.250 plc 2 decode_errors 1 buffered 4 output active".into()).unwrap();
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
        assert!(lab.stop().metrics.is_none());
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
                events.recv_timeout(Duration::from_secs(1)).unwrap(),
                expected
            );
        }
    }

    #[test]
    #[ignore = "requires the playback lab binary, a Windows output and free lab ports"]
    fn real_listener_starts_and_stops() {
        use std::time::{Duration, Instant};
        let output = sonimesh_platform::CpalOutput::new();
        let device = output.default_device_probe().unwrap().device.id;
        let mut lab = Lab::default();
        lab.start("127.0.0.1", Some(device.as_str()), 40, false)
            .unwrap();
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
        drop(std::net::TcpStream::connect("127.0.0.1:47610").unwrap());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let state = lab.refresh();
            if state.status == "Session failed; waiting for Android" {
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
        assert!(lab.start("127.0.0.1", None, 60, false).is_err());
        assert!(!lab.stop().running);
        assert!(lab.child.is_none());

        lab.start("127.0.0.1", Some("unavailable-output-id"), 60, false)
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while lab.refresh().running && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(!lab.refresh().running, "unavailable device must fail");
    }
}
