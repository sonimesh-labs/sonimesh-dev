//! Bounded supplier of fresh media sessions for authenticated reconnection.
//!
//! The receiver pins a UDP source endpoint only after a complete protected
//! admission, and changing that endpoint requires a fresh authenticated media
//! session with fresh key material rather than automatic address migration.
//! This module is the pairing/control-session owner of the one-time pairing
//! media root: it retains the root secret and, through [`supply`], issues a
//! fresh key context only for a media identity that the authenticated control
//! plane has supplied and whose full key context has never been issued before
//! within this pairing lifetime; callers pair the context with the root
//! borrow from [`root`] to install the session. The owner can explicitly
//! revoke the allocator to drop the root and clear its cached contexts. It
//! performs no TLS, QR, pairing-record consumption, or network I/O; the
//! authenticated transport
//! that selects the media identity remains external (ADR 0017). Callers that
//! also own the coordinator can use
//! [`MediaSessionAllocator::supply_for_authenticated_control_session`] to
//! reject stale transport tokens before consuming freshness capacity.
//!
//! [`supply`]: MediaSessionAllocator::supply
//! [`root`]: IssuedMediaSession::root

use core::fmt;

use sonimesh_protocol::{MediaDirection, MediaKeyContext, SessionSecret};

use crate::{
    ControlSessionToken, ControlStreamCoordinator, MediaStreamIdentity,
    PairingInvitationController, PairingInvitationError, PairingRecordState,
};

/// Fixed upper bound on the retained no-reuse record of issued key contexts.
///
/// The record is bounded like every queue in the codebase; when it fills, the
/// allocator fails closed and the operator must establish a fresh pairing root
/// instead of silently dropping the no-reuse guarantee.
pub const MAX_RETAINED_MEDIA_IDENTITIES: usize = 16;

/// Failure while issuing a fresh media session from the pairing media root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaSessionAllocatorError {
    /// The allocator was explicitly revoked and its pairing root was dropped.
    Revoked,
    /// The supplied control transport token is no longer active.
    ControlSessionNotActive,
    /// The identity is not a valid media key context (zero session or stream ID).
    InvalidIdentity,
    /// The full key context was already issued within this pairing lifetime.
    IdentityReused,
    /// The no-reuse record is full; a fresh pairing root is required.
    IdentityCapacityExceeded,
}

impl fmt::Display for MediaSessionAllocatorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Revoked => formatter.write_str("media session allocator has been revoked"),
            Self::ControlSessionNotActive => {
                formatter.write_str("authenticated control session is no longer active")
            }
            Self::InvalidIdentity => {
                formatter.write_str("media identity has a zero session or stream identifier")
            }
            Self::IdentityReused => {
                formatter.write_str("media key context was already issued in this pairing lifetime")
            }
            Self::IdentityCapacityExceeded => {
                formatter.write_str("media identity no-reuse record is full; re-pairing required")
            }
        }
    }
}

impl std::error::Error for MediaSessionAllocatorError {}

/// One fresh media context bound to the allocator's pairing root.
///
/// The allocator returns this grant from [`MediaSessionAllocator::supply`]
/// rather than exposing its root independently. Use [`Self::context`] and
/// [`Self::root`] together when constructing or replacing a protected media
/// pipeline. The root reference is borrowed from the allocator, so another
/// identity cannot be supplied while the grant remains live.
pub struct IssuedMediaSession<'a> {
    context: MediaKeyContext,
    root: &'a SessionSecret,
}

impl IssuedMediaSession<'_> {
    /// Returns the fresh context selected by the authenticated control domain.
    #[must_use]
    pub const fn context(&self) -> MediaKeyContext {
        self.context
    }

    /// Returns the pairing root bound to [`Self::context`].
    #[must_use]
    pub const fn root(&self) -> &SessionSecret {
        self.root
    }
}

impl fmt::Debug for IssuedMediaSession<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IssuedMediaSession")
            .field("context", &self.context)
            .field("root", &"[REDACTED]")
            .finish()
    }
}

/// Control-domain owner of the pairing media root and its freshness record.
///
/// A fresh pairing root replaces the whole allocator (drop the old instance and
/// construct a new one), which also resets the no-reuse record; the record must
/// never silently outlive the root it was derived from.
pub struct MediaSessionAllocator {
    root: Option<SessionSecret>,
    issued: [Option<MediaKeyContext>; MAX_RETAINED_MEDIA_IDENTITIES],
    issued_len: usize,
}

impl MediaSessionAllocator {
    /// Creates an owner for one authenticated pairing media root.
    #[must_use]
    pub const fn new(root: SessionSecret) -> Self {
        Self {
            root: Some(root),
            issued: [const { None }; MAX_RETAINED_MEDIA_IDENTITIES],
            issued_len: 0,
        }
    }

    /// Returns the number of media identities issued so far in this pairing.
    #[must_use]
    pub const fn issued_count(&self) -> usize {
        self.issued_len
    }

    /// Returns whether this pairing-root allocator has been revoked.
    #[must_use]
    pub const fn is_revoked(&self) -> bool {
        self.root.is_none()
    }

    /// Revokes the pairing root and clears the retained no-reuse record.
    ///
    /// The root is taken and dropped immediately, which invokes
    /// [`SessionSecret`]'s zeroization. Any live [`IssuedMediaSession`] grant
    /// prevents this mutable operation at compile time; callers must retire
    /// active media grants before revocation. The allocator cannot be reused
    /// after this call and every later supply attempt fails closed.
    pub fn revoke(&mut self) {
        let _ = self.root.take();
        self.issued = [const { None }; MAX_RETAINED_MEDIA_IDENTITIES];
        self.issued_len = 0;
    }

    /// Revokes a pairing record, its control session, and this allocator's root
    /// as one control-domain operation.
    ///
    /// This composes the pairing owner's terminal revocation transition with
    /// media-root retirement so callers cannot leave a trusted pairing record
    /// revoked while its allocator continues to issue sessions. The coordinator
    /// retires its control token, negotiated capabilities, active streams, and
    /// pending work before this method drops the root and clears its cached
    /// contexts.
    ///
    /// # Errors
    ///
    /// Returns [`PairingInvitationError`] without revoking this allocator when
    /// the pairing owner cannot perform the revocation transition.
    pub fn revoke_for_pairing<const MAX_SOURCES: usize>(
        &mut self,
        coordinator: &mut ControlStreamCoordinator,
        pairing: &mut PairingInvitationController<MAX_SOURCES>,
    ) -> Result<PairingRecordState, PairingInvitationError> {
        let state = coordinator.revoke_pairing(pairing)?;
        self.revoke();
        Ok(state)
    }

    /// Returns whether `context` was already issued in this pairing lifetime.
    #[must_use]
    pub fn contains(&self, context: MediaKeyContext) -> bool {
        self.issued[..self.issued_len]
            .iter()
            .any(|issued| *issued == Some(context))
    }

    /// Issues a fresh, root-bound media context for an authenticated identity.
    ///
    /// The caller supplies the identity selected by the authenticated control
    /// plane (for example from a `StartStream` or `Rotate` command after an
    /// authenticated reconnection). The allocator validates the context and
    /// rejects any context already issued in this pairing lifetime before
    /// recording it. On success it returns a grant that carries the matching
    /// root borrow; callers cannot obtain an allocator root without first
    /// holding a grant.
    ///
    /// # Errors
    ///
    /// Returns [`MediaSessionAllocatorError::Revoked`] when this allocator was
    /// explicitly revoked, [`MediaSessionAllocatorError::InvalidIdentity`] for
    /// a zero session or stream identifier, or
    /// [`MediaSessionAllocatorError::IdentityReused`]
    /// when the full key context was already issued, or
    /// [`MediaSessionAllocatorError::IdentityCapacityExceeded`] when the
    /// bounded no-reuse record is full.
    pub fn supply(
        &mut self,
        identity: MediaStreamIdentity,
        direction: MediaDirection,
    ) -> Result<IssuedMediaSession<'_>, MediaSessionAllocatorError> {
        let root = self
            .root
            .as_ref()
            .ok_or(MediaSessionAllocatorError::Revoked)?;
        let context = identity
            .media_key_context(direction)
            .map_err(|_| MediaSessionAllocatorError::InvalidIdentity)?;
        if self.contains(context) {
            return Err(MediaSessionAllocatorError::IdentityReused);
        }
        if self.issued_len == MAX_RETAINED_MEDIA_IDENTITIES {
            return Err(MediaSessionAllocatorError::IdentityCapacityExceeded);
        }
        self.issued[self.issued_len] = Some(context);
        self.issued_len += 1;
        Ok(IssuedMediaSession { context, root })
    }

    /// Issues a fresh media context only for the currently installed control
    /// transport.
    ///
    /// This is the composition entry point for callers that have both the
    /// coordinator and the pairing-root allocator. It checks the opaque
    /// transport token before touching the allocator's no-reuse record, so a
    /// delayed command from a retired connection cannot consume one of the
    /// bounded fresh-session slots. Existing grants are not revoked by this
    /// check; the pairing owner must still drop the allocator when the trust
    /// record expires or is revoked.
    ///
    /// # Errors
    ///
    /// Returns [`MediaSessionAllocatorError::ControlSessionNotActive`] before
    /// identity validation or freshness accounting when `session` is stale.
    pub fn supply_for_authenticated_control_session(
        &mut self,
        coordinator: &ControlStreamCoordinator,
        session: ControlSessionToken,
        identity: MediaStreamIdentity,
        direction: MediaDirection,
    ) -> Result<IssuedMediaSession<'_>, MediaSessionAllocatorError> {
        if !coordinator.has_authenticated_control_session(session) {
            return Err(MediaSessionAllocatorError::ControlSessionNotActive);
        }
        self.supply(identity, direction)
    }
}

impl fmt::Debug for MediaSessionAllocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaSessionAllocator")
            .field(
                "root",
                &if self.is_revoked() {
                    "[REVOKED]"
                } else {
                    "[REDACTED]"
                },
            )
            .field("issued_len", &self.issued_len)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use sonimesh_protocol::{MediaDirection, SessionSecret};

    use super::{MAX_RETAINED_MEDIA_IDENTITIES, MediaSessionAllocator, MediaSessionAllocatorError};
    use crate::{
        ControlSessionToken, ControlStreamCoordinator, MediaStreamIdentity, PairingAttemptOutcome,
        PairingId, PairingInvitationController, PairingInvitationError, PairingProofVerification,
        PairingRecordState, PairingSource, SessionState,
    };

    const DIRECTION: MediaDirection = MediaDirection::AndroidToDesktop;

    fn authenticated_pairing_owner() -> (
        ControlStreamCoordinator,
        ControlSessionToken,
        PairingInvitationController<1>,
    ) {
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
        (coordinator, token, pairing)
    }

    const fn identity(session_id: u64, stream_id: u32, key_epoch: u16) -> MediaStreamIdentity {
        MediaStreamIdentity {
            session_id,
            stream_id,
            key_epoch,
        }
    }

    #[test]
    fn revocation_zeroizes_the_root_boundary_and_clears_freshness_state() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        let context = allocator
            .supply(identity(7, 1, 0), DIRECTION)
            .expect("fresh identity")
            .context();
        assert!(allocator.contains(context));

        allocator.revoke();

        assert!(allocator.is_revoked());
        assert_eq!(allocator.issued_count(), 0);
        assert!(!allocator.contains(context));
        assert!(matches!(
            allocator.supply(identity(8, 1, 0), DIRECTION),
            Err(MediaSessionAllocatorError::Revoked)
        ));
        let debug = format!("{allocator:?}");
        assert!(debug.contains("[REVOKED]"));
        assert!(!debug.contains("A5"));
    }

    #[test]
    fn pairing_revocation_retires_control_and_media_root_together() {
        let (mut coordinator, token, mut pairing) = authenticated_pairing_owner();
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        let context = allocator
            .supply_for_authenticated_control_session(
                &coordinator,
                token,
                identity(7, 1, 0),
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
        assert_eq!(pairing.state(), Some(PairingRecordState::Revoked));
        assert!(matches!(
            allocator.supply(identity(8, 1, 0), DIRECTION),
            Err(MediaSessionAllocatorError::Revoked)
        ));
    }

    #[test]
    fn failed_pairing_revocation_preserves_the_media_root() {
        let mut coordinator = ControlStreamCoordinator::new(SessionState::IDLE, &[])
            .expect("bounded coordinator policy");
        let mut pairing = PairingInvitationController::<1>::new();
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        let context = allocator
            .supply(identity(7, 1, 0), DIRECTION)
            .expect("fresh identity")
            .context();

        assert_eq!(
            allocator.revoke_for_pairing(&mut coordinator, &mut pairing),
            Err(PairingInvitationError::MissingInvitation)
        );
        assert!(!allocator.is_revoked());
        assert_eq!(allocator.issued_count(), 1);
        assert!(allocator.contains(context));
    }

    #[test]
    fn authenticated_supply_rejects_a_retired_transport_before_consuming_capacity() {
        let (mut coordinator, token, _) = authenticated_pairing_owner();
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));

        {
            let grant = allocator
                .supply_for_authenticated_control_session(
                    &coordinator,
                    token,
                    identity(7, 1, 0),
                    DIRECTION,
                )
                .expect("active transport may issue a fresh session");
            assert_eq!(grant.context().session_id, 7);
        }
        assert_eq!(allocator.issued_count(), 1);

        coordinator.expire_control_session();
        let error = allocator
            .supply_for_authenticated_control_session(
                &coordinator,
                token,
                identity(8, 1, 0),
                DIRECTION,
            )
            .expect_err("retired transport must not issue a fresh session");

        assert_eq!(error, MediaSessionAllocatorError::ControlSessionNotActive);
        assert_eq!(allocator.issued_count(), 1);
    }

    #[test]
    fn issues_a_fresh_session_for_the_first_authenticated_identity() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        let id = identity(7, 1, 0);

        let context = allocator
            .supply(id, DIRECTION)
            .expect("fresh identity")
            .context();

        assert_eq!(context.session_id, 7);
        assert_eq!(context.stream_id, 1);
        assert_eq!(context.key_epoch, 0);
        assert_eq!(context.direction, DIRECTION);
        assert_eq!(allocator.issued_count(), 1);
        assert!(allocator.contains(context));
    }

    #[test]
    fn rejects_reusing_an_issued_context_within_the_pairing_lifetime() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        let id = identity(7, 1, 0);
        allocator.supply(id, DIRECTION).expect("fresh identity");

        let error = allocator
            .supply(id, DIRECTION)
            .expect_err("reuse must fail");

        assert_eq!(error, MediaSessionAllocatorError::IdentityReused);
        assert_eq!(allocator.issued_count(), 1);
    }

    #[test]
    fn rejects_a_zero_session_or_stream_identifier() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));

        for id in [identity(0, 1, 0), identity(1, 0, 0)] {
            let error = allocator
                .supply(id, DIRECTION)
                .expect_err("zero identity must fail");
            assert_eq!(error, MediaSessionAllocatorError::InvalidIdentity);
        }
        assert_eq!(allocator.issued_count(), 0);
    }

    #[test]
    fn a_new_epoch_for_the_same_session_is_a_fresh_context() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));

        let first = allocator
            .supply(identity(7, 1, 0), DIRECTION)
            .unwrap()
            .context();
        let rotated = allocator
            .supply(identity(7, 1, 1), DIRECTION)
            .expect("higher epoch is a fresh context")
            .context();

        assert_eq!(rotated.session_id, first.session_id);
        assert_eq!(rotated.key_epoch, 1);
        assert_eq!(allocator.issued_count(), 2);
    }

    #[test]
    fn a_new_session_id_is_a_fresh_context() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        allocator.supply(identity(7, 1, 0), DIRECTION).unwrap();

        let fresh = allocator
            .supply(identity(8, 1, 0), DIRECTION)
            .expect("new session id is a fresh context")
            .context();

        assert_eq!(fresh.session_id, 8);
    }

    #[test]
    fn direction_is_bound_into_the_no_reuse_context() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        allocator.supply(identity(7, 1, 0), DIRECTION).unwrap();

        let other_direction = allocator
            .supply(identity(7, 1, 0), MediaDirection::DesktopToAndroid)
            .expect("opposite direction is a distinct key context")
            .context();

        assert_eq!(other_direction.direction, MediaDirection::DesktopToAndroid);
        assert_eq!(allocator.issued_count(), 2);
    }

    #[test]
    fn grants_bind_each_context_to_the_retained_root_and_debug_is_redacted() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        let first_root = {
            let grant = allocator.supply(identity(7, 1, 0), DIRECTION).unwrap();
            let _ = grant.context();
            grant.root() as *const SessionSecret
        };
        let second_root = {
            let grant = allocator.supply(identity(8, 1, 0), DIRECTION).unwrap();
            grant.root() as *const SessionSecret
        };

        let debug = format!("{allocator:?}");
        assert!(!debug.contains("A5"));
        assert!(debug.contains("issued_len: 2"));
        assert_eq!(first_root, second_root);
    }

    #[test]
    fn the_no_reuse_record_is_bounded_and_fails_closed() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));

        for index in 0..MAX_RETAINED_MEDIA_IDENTITIES {
            allocator
                .supply(identity(10 + index as u64, 1, 0), DIRECTION)
                .unwrap_or_else(|error| panic!("identity {index} must be fresh: {error}"));
        }

        let error = allocator
            .supply(identity(999, 1, 0), DIRECTION)
            .expect_err("record full must fail closed");

        assert_eq!(error, MediaSessionAllocatorError::IdentityCapacityExceeded);
        assert_eq!(allocator.issued_count(), MAX_RETAINED_MEDIA_IDENTITIES);
    }
}
