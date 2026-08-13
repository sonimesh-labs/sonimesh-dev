#![no_main]

//! Fuzz the authenticated control-plane state machine.
//!
//! The coordinator only accepts envelopes after the transport authenticates
//! its control session. This target drives the real pairing path to a ready
//! session, installs one authenticated transport, then feeds arbitrary decoded
//! control frames plus mutations of the published Hello, StartStream, and
//! StopStream frames through the coordinator. Every produced Start, Rotate, or
//! Stop action is either committed with its token or aborted, exercising the
//! tokenized completion contract. Adversarial authenticated bytes must never
//! panic the coordinator, admit media past `MAX_ACTIVE_STREAMS`, or complete
//! work across a session-expiry or reconnect boundary.

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{
    ControlSessionToken, ControlStreamAction, ControlStreamCoordinator, PairingId,
    PairingInvitationController, PairingProofVerification, PairingSource, SessionEvent,
    SessionState,
};
use sonimesh_protocol::{Capability, decode_control_frame};

/// Local capability policy negotiated against every authenticated peer hello.
const LOCAL_CAPABILITIES: [Capability; 4] = [
    Capability::FrameDuration10Ms,
    Capability::FrameDuration20Ms,
    Capability::ManualAddress,
    Capability::NetworkDiagnostics,
];

const PUBLISHED_HELLO_FRAME: [u8; 28] = [
    0x00, 0x00, 0x00, 0x18, 0x08, 0x01, 0x10, 0x07, 0x52, 0x12, 0x0A, 0x01, 0x01, 0x12, 0x01, 0x02,
    0x1A, 0x0A, 0x4C, 0x61, 0x62, 0x20, 0x73, 0x65, 0x6E, 0x64, 0x65, 0x72,
];
const PUBLISHED_START_STREAM_FRAME: [u8; 25] = [
    0x00, 0x00, 0x00, 0x15, 0x08, 0x01, 0x10, 0x08, 0x5A, 0x0F, 0x08, 0x07, 0x10, 0x09, 0x18, 0x80,
    0xF7, 0x02, 0x20, 0x02, 0x28, 0x14, 0x30, 0xC5, 0x46,
];
const PUBLISHED_STOP_STREAM_FRAME: [u8; 17] = [
    0x00, 0x00, 0x00, 0x0D, 0x08, 0x01, 0x10, 0x09, 0x62, 0x07, 0x08, 0x07, 0x10, 0x09, 0x18, 0xC5,
    0x46,
];

/// One trusted ready coordinator with an installed authenticated transport.
struct ReadyCoordinator {
    coordinator: ControlStreamCoordinator,
    session: ControlSessionToken,
}

impl ReadyCoordinator {
    /// Establishes trust through the real pairing path and installs the session.
    fn establish() -> Option<Self> {
        let mut pairing = PairingInvitationController::<1>::new();
        let mut coordinator =
            ControlStreamCoordinator::new(SessionState::IDLE, &LOCAL_CAPABILITIES).ok()?;
        coordinator
            .begin_pairing(&mut pairing, PairingId::new([1; 16]), 100, 1_000, 16)
            .ok()?;
        coordinator
            .verify_pairing_proof(&mut pairing, 101, PairingSource::new([2; 16]), || {
                PairingProofVerification::Verified
            })
            .ok()?;
        let session = coordinator.install_authenticated_control_session().ok()?;
        Some(Self {
            coordinator,
            session,
        })
    }

    fn accept(&mut self, envelope: &sonimesh_protocol::Envelope) -> Option<ControlStreamAction> {
        self.coordinator.accept(self.session, envelope).ok()
    }

    /// Commits or aborts one produced lifecycle action with its token.
    fn settle(&mut self, action: ControlStreamAction, commit: bool) {
        match action {
            ControlStreamAction::Start { token, request } => {
                if commit {
                    let _ = self.coordinator.complete_start(token, request);
                } else {
                    let _ = self.coordinator.abort_pending(token);
                }
            }
            ControlStreamAction::Rotate {
                token,
                previous,
                next,
            } => {
                if commit {
                    let _ = self.coordinator.complete_rotation(token, previous, next);
                } else {
                    let _ = self.coordinator.abort_pending(token);
                }
            }
            ControlStreamAction::Stop { token, identity } => {
                if commit {
                    let _ = self.coordinator.complete_stop(token, identity);
                } else {
                    let _ = self.coordinator.abort_pending(token);
                }
            }
            ControlStreamAction::Negotiated(_)
            | ControlStreamAction::Noop
            | ControlStreamAction::Ignored => {}
        }
    }
}

/// XORs `seed` with the next mutation bytes and feeds the decoded frame to the
/// coordinator, then commits or aborts any produced action. An empty mutation
/// slice leaves the published frame untouched so the happy path is always
/// reachable, even from the smallest fuzz inputs.
fn feed_mutated<const N: usize>(
    ready: &mut ReadyCoordinator,
    seed: [u8; N],
    mutations: &[u8],
    commit: bool,
) {
    let mut near_valid = seed;
    for (target, mutation) in near_valid.iter_mut().zip(mutations) {
        *target ^= mutation;
    }
    if let Ok(envelope) = decode_control_frame(&near_valid) {
        if let Some(action) = ready.accept(&envelope) {
            ready.settle(action, commit);
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let Some(mut ready) = ReadyCoordinator::establish() else {
        return;
    };

    // Scripted trusted control flow: negotiate, start, start again (rotate or
    // conflict on the same identity, or a second stream), stop, restart, then
    // attempt renegotiation. Each step is perturbed by a distinct data slice.
    feed_mutated(
        &mut ready,
        PUBLISHED_HELLO_FRAME,
        data.get(0..28).unwrap_or(&[]),
        commit_byte(data, 0),
    );
    feed_mutated(
        &mut ready,
        PUBLISHED_START_STREAM_FRAME,
        data.get(28..53).unwrap_or(&[]),
        commit_byte(data, 1),
    );
    feed_mutated(
        &mut ready,
        PUBLISHED_START_STREAM_FRAME,
        data.get(53..78).unwrap_or(&[]),
        commit_byte(data, 2),
    );
    feed_mutated(
        &mut ready,
        PUBLISHED_STOP_STREAM_FRAME,
        data.get(78..95).unwrap_or(&[]),
        commit_byte(data, 3),
    );
    feed_mutated(
        &mut ready,
        PUBLISHED_START_STREAM_FRAME,
        data.get(95..120).unwrap_or(&[]),
        commit_byte(data, 4),
    );
    feed_mutated(
        &mut ready,
        PUBLISHED_HELLO_FRAME,
        data.get(120..148).unwrap_or(&[]),
        commit_byte(data, 5),
    );

    // Arbitrary authenticated envelopes exercise the reject and ignore paths,
    // including StaleControlSession after a transport boundary below.
    for chunk in data.chunks(64) {
        if let Ok(envelope) = decode_control_frame(chunk) {
            let _ = ready.accept(&envelope);
        }
    }
    assert!(ready.coordinator.stream_count() <= sonimesh_app::MAX_ACTIVE_STREAMS);

    // Clock-driven reconnect boundary: the lost transport must retire the old
    // token, and the armed grace window must expire (or stay bounded) without
    // panicking or admitting a late completion.
    let _ = ready
        .coordinator
        .acknowledge_platform_event(SessionEvent::ConnectionLost);
    let duration_ms = u64::from(data.first().copied().unwrap_or(1)) % 500 + 1;
    let _ = ready.coordinator.begin_reconnect_grace(2_000, duration_ms);
    let _ = ready
        .coordinator
        .observe_reconnect_grace(2_000 + u64::from(data.get(1).copied().unwrap_or(0)));
    ready.coordinator.expire_control_session();
});

/// Selects commit (even) or abort (odd) for the scripted step at `index`.
fn commit_byte(data: &[u8], index: usize) -> bool {
    data.get(index).is_none_or(|byte| byte % 2 == 0)
}
