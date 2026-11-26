//! Exercise the actual lab executable through pairing, stream startup and UDP.
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use sonimesh_network::{MediaSessionSender, client_config, connect_pinned_client};
use sonimesh_protocol::{
    Capability, Envelope, Hello, MAX_CONTROL_FRAME_BYTES, MediaDirection, MediaKeyContext,
    PROOF_FRAME_LEN, PairingBinding, PairingProofFrame, PairingRole, StartStream, StopStream,
    StreamState, decode_control_frame, encode_control_frame, envelope, parse_pairing_proof_frame,
};

struct Listener(Child);

impl Drop for Listener {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn reply(stream: &mut impl Read) -> Envelope {
    let mut prefix = [0; 4];
    stream.read_exact(&mut prefix).unwrap();
    let length = u32::from_be_bytes(prefix) as usize;
    assert!(length <= MAX_CONTROL_FRAME_BYTES);
    let mut frame = Vec::from(prefix);
    frame.resize(4 + length, 0);
    stream.read_exact(&mut frame[4..]).unwrap();
    decode_control_frame(&frame).unwrap()
}

fn send(stream: &mut impl Write, envelope: &Envelope) {
    stream
        .write_all(&encode_control_frame(envelope).unwrap())
        .unwrap();
    stream.flush().unwrap();
}

#[test]
fn executable_pairs_and_receives_both_frame_durations() {
    exercise_listener(&[10, 20], false, false);
}

#[test]
fn executable_detects_probe_after_protected_udp_and_decode() {
    exercise_listener(&[20], true, false);
}

#[cfg(all(windows, feature = "playback"))]
#[test]
#[ignore = "requires a Windows output device"]
fn executable_acknowledges_output_switches_during_a_session() {
    exercise_listener(&[20], false, true);
}

#[allow(clippy::too_many_lines)] // Keep the pairing and media exchange in one visible scenario.
fn exercise_listener(durations: &[u32], check_probe: bool, playback_control: bool) {
    for &duration in durations {
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_port = tcp.local_addr().unwrap().port();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let media_port = udp.local_addr().unwrap().port();
        drop((tcp, udp));

        let mut command = Command::new(env!("CARGO_BIN_EXE_paired-audio-lab"));
        command.args([
            "--control-port",
            &control_port.to_string(),
            "--media-port",
            &media_port.to_string(),
            "--startup-buffer-ms",
            "120",
            "--adaptive",
            "--require-tone",
            "--status",
        ]);
        if check_probe {
            command.arg("--check-probe");
        }
        if playback_control {
            command.args(["--play", "--control"]).stdin(Stdio::piped());
        }
        let mut child = Listener(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let output = child.0.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if sender.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut pin = None;
        loop {
            let line = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("listener did not become ready");
            if let Some(hex) = line.strip_prefix("SPKI_HEX ") {
                pin = Some(
                    hex.as_bytes()
                        .chunks_exact(2)
                        .map(|pair| {
                            u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()
                        })
                        .collect::<Vec<_>>(),
                );
            }
            if line == "READY" {
                break;
            }
        }
        let id = [0x71; 16];
        let socket = TcpStream::connect(("127.0.0.1", control_port)).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (mut channel, exporter) = connect_pinned_client(
            client_config(pin.unwrap()).unwrap(),
            "localhost",
            &id,
            socket,
        )
        .unwrap();
        let binding = PairingBinding::new(1, id, [0x42; 32], exporter.as_ref()).unwrap();
        let proof = PairingProofFrame {
            protocol_version: 1,
            role: PairingRole::AndroidClient,
            pairing_id: id,
            proof: binding.local_proof(PairingRole::AndroidClient).unwrap(),
        };
        let mut bytes = [0; PROOF_FRAME_LEN];
        proof.encode_into(&mut bytes).unwrap();
        channel.write_all(&bytes).unwrap();
        channel.flush().unwrap();
        channel.read_exact(&mut bytes).unwrap();
        let proof = parse_pairing_proof_frame(&bytes).unwrap();
        assert!(
            binding
                .verify_peer_proof(PairingRole::AndroidClient, &proof.proof)
                .unwrap()
        );
        let hello = Envelope {
            protocol_version: 1,
            request_id: 1,
            payload: Some(envelope::Payload::Hello(Hello {
                supported_versions: vec![1],
                capabilities: vec![
                    Capability::FrameDuration10Ms as i32,
                    Capability::FrameDuration20Ms as i32,
                ],
                display_name: "test".into(),
            })),
        };
        send(&mut channel, &hello);
        assert!(matches!(
            reply(&mut channel).payload,
            Some(envelope::Payload::Hello(_))
        ));
        let start = Envelope {
            protocol_version: 1,
            request_id: 2,
            payload: Some(envelope::Payload::StartStream(StartStream {
                media_session_id: 7,
                stream_id: 1,
                sample_rate_hz: 48_000,
                channels: 2,
                frame_duration_ms: duration,
                media_key_epoch: 0,
            })),
        };
        send(&mut channel, &start);
        assert!(
            matches!(reply(&mut channel).payload, Some(envelope::Payload::StreamStatus(status)) if status.state == StreamState::Streaming as i32)
        );
        if playback_control {
            let session = loop {
                let line = receiver
                    .recv_timeout(Duration::from_secs(10))
                    .expect("output session");
                if let Some(session) = line.strip_prefix("OUTPUT-SESSION ") {
                    break session.parse::<u64>().unwrap();
                }
            };
            let input = child.0.stdin.as_mut().unwrap();
            writeln!(input, "output 9 {} 0", session + 1).unwrap();
            input.flush().unwrap();
            loop {
                let line = receiver
                    .recv_timeout(Duration::from_secs(10))
                    .expect("stale acknowledgement");
                if line.starts_with("OUTPUT-SWITCH 9 ") {
                    assert_eq!(line, "OUTPUT-SWITCH 9 inactive");
                    break;
                }
            }
            let input = child.0.stdin.as_mut().unwrap();
            writeln!(input, "output 1 {session} 0").unwrap();
            input.flush().unwrap();
            loop {
                let line = receiver
                    .recv_timeout(Duration::from_secs(10))
                    .expect("switch acknowledgement");
                if line.starts_with("OUTPUT-SWITCH 1 ") {
                    assert_eq!(line, "OUTPUT-SWITCH 1 applied");
                    break;
                }
            }
            let input = child.0.stdin.as_mut().unwrap();
            write!(input, "output 2 {session} 7\ninvalid").unwrap();
            input.flush().unwrap();
            loop {
                let line = receiver
                    .recv_timeout(Duration::from_secs(12))
                    .expect("failed switch acknowledgement");
                if line.starts_with("OUTPUT-SWITCH 2 ") {
                    assert_eq!(line, "OUTPUT-SWITCH 2 unavailable");
                    break;
                }
            }
        }
        let context = MediaKeyContext::new(7, 1, 0, MediaDirection::AndroidToDesktop).unwrap();
        let mut sender = MediaSessionSender::bind(
            "127.0.0.1:0".parse().unwrap(),
            &binding.media_root().unwrap(),
            context,
            format!("127.0.0.1:{media_port}").parse().unwrap(),
        )
        .unwrap();
        let mut encoder =
            opus::Encoder::new(48_000, opus::Channels::Stereo, opus::Application::Audio).unwrap();
        let mut packet = [0; 1_148];
        if check_probe {
            let reference = sonimesh_latency_measurement::test_signal(48_000, 3, 7);
            let start_time = Instant::now();
            for (sequence, frame) in reference.chunks_exact(duration as usize * 48).enumerate() {
                let pcm: Vec<_> = frame
                    .iter()
                    .flat_map(|sample| [sample * 0.0915; 2])
                    .collect();
                let length = encoder.encode_float(&pcm, &mut packet).unwrap();
                let deadline =
                    start_time + Duration::from_millis(sequence as u64 * u64::from(duration));
                std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
                sender
                    .send_frame(
                        0,
                        sequence as u64 * u64::from(duration) * 48,
                        &packet[..length],
                    )
                    .unwrap();
            }
        } else {
            let pcm = vec![0.2_f32; duration as usize * 96];
            for sequence in 0..13 {
                let length = encoder.encode_float(&pcm, &mut packet).unwrap();
                sender
                    .send_frame(0, sequence * u64::from(duration) * 48, &packet[..length])
                    .unwrap();
            }
        }
        std::thread::sleep(Duration::from_millis(150));
        let stop = Envelope {
            protocol_version: 1,
            request_id: 3,
            payload: Some(envelope::Payload::StopStream(StopStream {
                media_session_id: 7,
                stream_id: 1,
                media_key_epoch: 0,
            })),
        };
        send(&mut channel, &stop);
        assert!(
            matches!(reply(&mut channel).payload, Some(envelope::Payload::StreamStatus(status)) if status.state == StreamState::Stopped as i32)
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success(), "listener failed: {status}");
                break;
            }
            assert!(Instant::now() < deadline, "listener did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
        if check_probe {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let line = receiver
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .expect("listener did not report probe detection");
                if line == "RESULT latency probe detected" {
                    break;
                }
            }
        }
    }
}
