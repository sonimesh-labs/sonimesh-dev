#![no_main]

//! Fuzz the bounded media-session allocator no-reuse record.
//!
//! `MediaSessionAllocator` (ADR 0017) issues a fresh media key context only for
//! an identity the authenticated control plane supplies whose full key context
//! (session, stream, epoch, direction) was never issued before in the pairing
//! lifetime, and fails closed when its bounded 16-entry record is full. This
//! target drives arbitrary identity/direction supply scripts against an
//! independent append-only set model, proving the allocator cannot panic,
//! cannot re-issue a context within a pairing lifetime, cannot exceed the
//! bound, cannot record a failed supply, and always keeps its count and
//! `contains` view in lock-step with the model. Every input also checks that
//! an active control token can issue a grant while a retired token cannot
//! reserve a fresh identity or consume capacity.

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{
    ControlSessionToken, ControlStreamCoordinator, MAX_RETAINED_MEDIA_IDENTITIES,
    MediaSessionAllocator, MediaSessionAllocatorError, MediaStreamIdentity, PairingAttemptOutcome,
    PairingId, PairingInvitationController, PairingProofVerification, PairingRecordState,
    PairingSource, SessionState,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, SessionSecret};

const DIRECTION: MediaDirection = MediaDirection::AndroidToDesktop;
const DIRECTIONS: [MediaDirection; 2] = [
    MediaDirection::AndroidToDesktop,
    MediaDirection::DesktopToAndroid,
];

/// Reads one little-endian `u64` from `data`, wrapping around the slice.
fn next_u64(data: &[u8], offset: &mut usize) -> u64 {
    let mut bytes = [0_u8; 8];
    for byte in &mut bytes {
        *byte = data[*offset % data.len()];
        *offset += 1;
    }
    u64::from_le_bytes(bytes)
}

/// Reads one little-endian `u32` from `data`, wrapping around the slice.
fn next_u32(data: &[u8], offset: &mut usize) -> u32 {
    let mut bytes = [0_u8; 4];
    for byte in &mut bytes {
        *byte = data[*offset % data.len()];
        *offset += 1;
    }
    u32::from_le_bytes(bytes)
}

fn direction_from(data: &[u8], offset: &mut usize) -> MediaDirection {
    DIRECTIONS[usize::from(data[*offset % data.len()] & 1)]
}

const fn identity(session_id: u64, stream_id: u32, key_epoch: u16) -> MediaStreamIdentity {
    MediaStreamIdentity {
        session_id,
        stream_id,
        key_epoch,
    }
}

fn authenticated_coordinator() -> (ControlStreamCoordinator, ControlSessionToken) {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut state = SessionState::IDLE;
    pairing
        .begin(&mut state, PairingId::new([1; 16]), 100, 50, 1)
        .expect("pairing invitation begins");
    assert_eq!(
        pairing.verify_proof(&mut state, 101, PairingSource::new([2; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );

    let mut coordinator =
        ControlStreamCoordinator::new(state, &[]).expect("bounded coordinator policy");
    let token = coordinator
        .install_authenticated_control_session()
        .expect("authenticated transport token");
    (coordinator, token)
}

fn assert_control_session_gate() {
    let (mut coordinator, token) = authenticated_coordinator();
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0x5A; 32]));

    let context = allocator
        .supply_for_authenticated_control_session(&coordinator, token, identity(7, 1, 0), DIRECTION)
        .expect("active transport may issue a fresh identity")
        .context();
    assert_eq!(context.session_id, 7);
    assert_eq!(allocator.issued_count(), 1);

    coordinator.expire_control_session();
    assert!(matches!(
        allocator.supply_for_authenticated_control_session(
            &coordinator,
            token,
            identity(8, 1, 0),
            DIRECTION,
        ),
        Err(MediaSessionAllocatorError::ControlSessionNotActive)
    ));
    assert_eq!(allocator.issued_count(), 1);
}

fn assert_allocator_revocation() {
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xC3; 32]));
    let context = allocator
        .supply(identity(9, 1, 0), DIRECTION)
        .expect("fresh identity")
        .context();
    assert!(allocator.contains(context));

    allocator.revoke();

    assert!(allocator.is_revoked());
    assert_eq!(allocator.issued_count(), 0);
    assert!(!allocator.contains(context));
    assert!(matches!(
        allocator.supply(identity(10, 1, 0), DIRECTION),
        Err(MediaSessionAllocatorError::Revoked)
    ));
}

fn assert_pairing_revocation_composition() {
    let mut pairing = PairingInvitationController::<1>::new();
    let mut state = SessionState::IDLE;
    pairing
        .begin(&mut state, PairingId::new([3; 16]), 100, 50, 1)
        .expect("pairing invitation begins");
    assert_eq!(
        pairing.verify_proof(&mut state, 101, PairingSource::new([4; 16]), || {
            PairingProofVerification::Verified
        }),
        Ok(PairingAttemptOutcome::Paired)
    );

    let mut coordinator =
        ControlStreamCoordinator::new(state, &[]).expect("bounded coordinator policy");
    let token = coordinator
        .install_authenticated_control_session()
        .expect("authenticated transport token");
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xD4; 32]));
    let context = allocator
        .supply_for_authenticated_control_session(
            &coordinator,
            token,
            identity(11, 1, 0),
            DIRECTION,
        )
        .expect("active transport may issue a fresh identity")
        .context();

    assert_eq!(
        allocator.revoke_for_pairing(&mut coordinator, &mut pairing),
        Ok(PairingRecordState::Revoked)
    );
    assert!(allocator.is_revoked());
    assert_eq!(allocator.issued_count(), 0);
    assert!(!allocator.contains(context));
    assert_eq!(coordinator.state(), SessionState::IDLE);
    assert!(!coordinator.has_authenticated_control_session(token));
}

/// Allocator plus an independent append-only set model of the same record.
struct AllocatorModel {
    allocator: MediaSessionAllocator,
    issued: Vec<MediaKeyContext>,
}

impl AllocatorModel {
    fn new() -> Self {
        Self {
            allocator: MediaSessionAllocator::new(SessionSecret::new([0xA5; 32])),
            issued: Vec::new(),
        }
    }

    fn supply(&mut self, identity: MediaStreamIdentity, direction: MediaDirection) {
        let expected = match identity.media_key_context(direction) {
            Ok(context) if self.issued.contains(&context) => {
                Err(MediaSessionAllocatorError::IdentityReused)
            }
            Ok(_) if self.issued.len() == MAX_RETAINED_MEDIA_IDENTITIES => {
                Err(MediaSessionAllocatorError::IdentityCapacityExceeded)
            }
            Ok(context) => Ok(context),
            Err(_) => Err(MediaSessionAllocatorError::InvalidIdentity),
        };
        let actual = self
            .allocator
            .supply(identity, direction)
            .map(|grant| grant.context());
        assert_eq!(actual, expected, "supply diverged from the no-reuse model");
        if let Ok(context) = actual {
            self.issued.push(context);
        }
        assert_eq!(
            self.allocator.issued_count(),
            self.issued.len(),
            "issued count drifted from the model"
        );
        assert!(
            self.issued
                .iter()
                .all(|issued| self.allocator.contains(*issued)),
            "an issued context vanished from the allocator record"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    assert_control_session_gate();
    assert_allocator_revocation();
    assert_pairing_revocation_composition();
    if data.is_empty() {
        return;
    }
    let mut model = AllocatorModel::new();

    // Deterministic edge coverage on every run: fresh issue, zero-identifier
    // rejection, exact replay, epoch rotation, new session, opposite
    // direction, capacity fill, and the 17th distinct identity failing closed.
    model.supply(identity(7, 1, 0), DIRECTION);
    model.supply(identity(0, 1, 0), DIRECTION);
    model.supply(identity(1, 0, 0), DIRECTION);
    model.supply(identity(7, 1, 0), DIRECTION);
    model.supply(identity(7, 1, 1), DIRECTION);
    model.supply(identity(8, 1, 0), DIRECTION);
    model.supply(identity(7, 1, 0), MediaDirection::DesktopToAndroid);
    for index in 0..(MAX_RETAINED_MEDIA_IDENTITIES - 4) {
        model.supply(identity(100 + index as u64, 1, 0), DIRECTION);
    }
    model.supply(identity(999, 1, 0), DIRECTION);

    // Fuzz-derived supplies with adversarial identities and directions.
    let mut offset = 0;
    let steps = 1 + data.len() % 64;
    for _ in 0..steps {
        let op = data[offset % data.len()];
        offset += 1;
        match op % 4 {
            0 => {
                let session_id = next_u64(data, &mut offset);
                let stream_id = next_u32(data, &mut offset);
                let key_epoch = (next_u32(data, &mut offset) & 0xFFFF) as u16;
                model.supply(
                    identity(session_id, stream_id, key_epoch),
                    direction_from(data, &mut offset),
                );
            }
            1 => {
                if let Some(context) = model.issued.first() {
                    let replay = identity(context.session_id, context.stream_id, context.key_epoch);
                    model.supply(replay, context.direction);
                }
            }
            2 => {
                let zero_session = op & 2 == 0;
                if zero_session {
                    model.supply(identity(0, next_u32(data, &mut offset) | 1, 0), DIRECTION);
                } else {
                    model.supply(
                        identity(u64::from(next_u32(data, &mut offset)) | 1, 0, 0),
                        DIRECTION,
                    );
                }
            }
            _ => {
                let session_id = u64::from(next_u32(data, &mut offset) % 3) + 1;
                let stream_id = next_u32(data, &mut offset) % 4 + 1;
                let key_epoch = (next_u32(data, &mut offset) % 3) as u16;
                model.supply(
                    identity(session_id, stream_id, key_epoch),
                    direction_from(data, &mut offset),
                );
            }
        }
    }
});
