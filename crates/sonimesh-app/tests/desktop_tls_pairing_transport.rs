//! End-to-end pairing transport over a real pinned TLS channel.
//!
//! This test closes the gap between the ADR 0021 provider boundary and the
//! application-domain handshake: a real Rustls TLS 1.3 connection (fresh lab
//! certificate, QR-carried SPKI pin) supplies the live exporter that drives
//! the pure desktop composition (`begin_from_rendered_payload`,
//! `verify_scanned_proof`), and both sides' media roots seal identical
//! datagrams. Both roles run in one process over loopback; no Android
//! runtime, camera scan, or persistent trust is claimed.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use rcgen::generate_simple_self_signed;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use zeroize::{Zeroize, Zeroizing};

use sonimesh_app::{
    PairingHandshakeOutcome, PairingId, PairingInvitationController, PairingQrInvitation,
    PairingSource, ScannedProofAttempt, SessionState, begin_from_rendered_payload,
    verify_scanned_proof,
};
use sonimesh_network::{accept_pinned_server, client_config, connect_pinned_client, server_config};
use sonimesh_protocol::{MAX_QR_PAYLOAD_LEN, PROOF_FRAME_LEN, PairingRole};

const HOST: &str = "localhost";
/// Fixed non-secret lab secret; never printed and dropped with the process.
const LAB_PAIRING_SECRET: [u8; 32] = [0xB7_u8; 32];

type TestResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[test]
fn live_pinned_tls_drives_the_desktop_pairing_composition() -> TestResult<()> {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let control_port = address.port();
    let invitation_id = [0x6A_u8; 16];

    // Fresh lab certificate whose public key becomes the QR-carried pin.
    let certified_key =
        generate_simple_self_signed(vec![HOST.to_owned()]).expect("lab certificate generates");
    let certificate = certified_key.cert.der().clone();
    let spki_pin = certified_key.key_pair.public_key_der();
    let mut private_key_der = Zeroizing::new(certified_key.key_pair.serialize_der());
    let private_key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(private_key_der.to_vec()));
    private_key_der.zeroize();

    // The desktop renders its invitation exactly as the production owner
    // would, embedding the certificate's SPKI as the pin.
    let mut pairing = PairingInvitationController::<4>::new();
    let mut session = SessionState::IDLE;
    let mut payload_out = vec![0_u8; MAX_QR_PAYLOAD_LEN];
    let written = begin_from_rendered_payload(
        &mut pairing,
        &mut session,
        100,
        60_000,
        2,
        PairingQrInvitation {
            id: PairingId::new(invitation_id),
            pairing_secret: LAB_PAIRING_SECRET,
            control_host: HOST,
            control_port,
            desktop_tls_spki: &spki_pin,
            expires_at_ms: 61_000,
        },
        &mut payload_out,
    )
    .expect("canonical invitation renders");
    payload_out.truncate(written);

    // Desktop thread: accept the pinned TLS connection, export with the
    // pairing context, read the peer frame from that channel, then verify
    // the scanned proof through the real owner.
    let server_config =
        Arc::new(server_config(certificate, private_key).expect("server config accepts own key"));
    let desktop_payload = payload_out.clone();
    let desktop = thread::spawn(move || -> TestResult<()> {
        let (stream, _) = listener.accept()?;
        let (mut channel, exporter) =
            accept_pinned_server(Arc::clone(&server_config), &invitation_id, stream)?;

        let mut wire = [0_u8; PROOF_FRAME_LEN];
        channel.read_exact(&mut wire)?;

        let outcome = verify_scanned_proof(
            &mut pairing,
            &mut session,
            150,
            PairingSource::new([0xC4; 16]),
            exporter.as_ref(),
            ScannedProofAttempt {
                scanned_payload: &desktop_payload,
                expected_pairing_id: PairingId::new(invitation_id),
                pairing_secret: LAB_PAIRING_SECRET,
                peer_proof_frame: &wire,
            },
        )?;

        assert!(matches!(outcome, PairingHandshakeOutcome::Ready { .. }));
        Ok(())
    });

    // Android-role side in this process: connect with the QR pin, derive the
    // client proof from the real exported bytes, send it into the channel.
    let stream = TcpStream::connect(address)?;
    let config = client_config(spki_pin.clone()).expect("pin builds a client config");
    let (mut client_channel, client_exporter) =
        connect_pinned_client(config, HOST, &invitation_id, stream)?;

    let binding = sonimesh_protocol::PairingBinding::new(
        1,
        invitation_id,
        LAB_PAIRING_SECRET,
        client_exporter.as_ref(),
    )
    .expect("live exporter derives the peer binding");
    let frame = sonimesh_protocol::PairingProofFrame {
        protocol_version: 1,
        role: PairingRole::AndroidClient,
        pairing_id: invitation_id,
        proof: binding
            .local_proof(PairingRole::AndroidClient)
            .expect("proof derivation succeeds"),
    };
    let mut wire = [0_u8; PROOF_FRAME_LEN];
    frame.encode_into(&mut wire).expect("frame encodes");
    client_channel.write_all(&wire)?;
    client_channel.flush()?;

    desktop.join().expect("desktop thread joins")?;
    Ok(())
}
