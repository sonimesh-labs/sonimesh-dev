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
#[allow(clippy::too_many_lines)] // Keep the pairing and media exchange in one visible scenario.
fn executable_pairs_and_receives_both_frame_durations() {
    for duration in [10_u32, 20] {
        let tcp = TcpListener::bind("127.0.0.1:0").unwrap();
        let control_port = tcp.local_addr().unwrap().port();
        let udp = UdpSocket::bind("127.0.0.1:0").unwrap();
        let media_port = udp.local_addr().unwrap().port();
        drop((tcp, udp));

        let mut child = Listener(
            Command::new(env!("CARGO_BIN_EXE_paired-audio-lab"))
                .args([
                    "--control-port",
                    &control_port.to_string(),
                    "--media-port",
                    &media_port.to_string(),
                    "--startup-buffer-ms",
                    "120",
                    "--adaptive",
                    "--require-tone",
                    "--status",
                ])
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
        let pcm = vec![0.2_f32; duration as usize * 96];
        let mut packet = [0; 1_148];
        for sequence in 0..13 {
            let length = encoder.encode_float(&pcm, &mut packet).unwrap();
            sender
                .send_frame(0, sequence * u64::from(duration) * 48, &packet[..length])
                .unwrap();
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
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                assert!(status.success(), "listener failed: {status}");
                break;
            }
            assert!(Instant::now() < deadline, "listener did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
