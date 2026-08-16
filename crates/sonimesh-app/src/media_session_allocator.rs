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
//! borrow from [`root`] to install the session. It performs no TLS, QR,
//! pairing-record consumption, or network I/O; the authenticated transport
//! that selects the media identity remains external (ADR 0017).
//!
//! [`supply`]: MediaSessionAllocator::supply
//! [`root`]: MediaSessionAllocator::root

use core::fmt;

use sonimesh_protocol::{MediaDirection, MediaKeyContext, SessionSecret};

use crate::MediaStreamIdentity;

/// Fixed upper bound on the retained no-reuse record of issued key contexts.
///
/// The record is bounded like every queue in the codebase; when it fills, the
/// allocator fails closed and the operator must establish a fresh pairing root
/// instead of silently dropping the no-reuse guarantee.
pub const MAX_RETAINED_MEDIA_IDENTITIES: usize = 16;

/// Failure while issuing a fresh media session from the pairing media root.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaSessionAllocatorError {
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

/// One freshly supplied media key context together with the retained root.
///
/// Callers obtain the context from [`MediaSessionAllocator::supply`] and the
/// root borrow from [`MediaSessionAllocator::root`], then hand both to
/// `ReceiverStreamPipeline::install_fresh_session` (or initial pipeline
/// construction). The root borrow lasts only for the call, so no long-lived
/// borrow of the allocator is required.
/// Control-domain owner of the pairing media root and its freshness record.
///
/// A fresh pairing root replaces the whole allocator (drop the old instance and
/// construct a new one), which also resets the no-reuse record; the record must
/// never silently outlive the root it was derived from.
pub struct MediaSessionAllocator {
    root: SessionSecret,
    issued: [Option<MediaKeyContext>; MAX_RETAINED_MEDIA_IDENTITIES],
    issued_len: usize,
}

impl MediaSessionAllocator {
    /// Creates an owner for one authenticated pairing media root.
    #[must_use]
    pub const fn new(root: SessionSecret) -> Self {
        Self {
            root,
            issued: [const { None }; MAX_RETAINED_MEDIA_IDENTITIES],
            issued_len: 0,
        }
    }

    /// Returns the number of media identities issued so far in this pairing.
    #[must_use]
    pub const fn issued_count(&self) -> usize {
        self.issued_len
    }

    /// Returns whether `context` was already issued in this pairing lifetime.
    #[must_use]
    pub fn contains(&self, context: MediaKeyContext) -> bool {
        self.issued[..self.issued_len]
            .iter()
            .any(|issued| *issued == Some(context))
    }

    /// Returns the retained pairing media root for one authenticated identity.
    ///
    /// The root is shared by every media session in this pairing lifetime; pair
    /// it with the context returned by [`Self::supply`] to install or rotate a
    /// stream in the receiver pipeline.
    #[must_use]
    pub const fn root(&self) -> &SessionSecret {
        &self.root
    }

    /// Issues a fresh media key context for an authenticated media identity.
    ///
    /// The caller supplies the identity selected by the authenticated control
    /// plane (for example from a `StartStream` or `Rotate` command after an
    /// authenticated reconnection). The allocator validates the context and
    /// rejects any context already issued in this pairing lifetime before
    /// recording it, so the caller can install it with [`Self::root`].
    ///
    /// # Errors
    ///
    /// Returns [`MediaSessionAllocatorError::InvalidIdentity`] for a zero
    /// session or stream identifier, [`MediaSessionAllocatorError::IdentityReused`]
    /// when the full key context was already issued, or
    /// [`MediaSessionAllocatorError::IdentityCapacityExceeded`] when the
    /// bounded no-reuse record is full.
    pub fn supply(
        &mut self,
        identity: MediaStreamIdentity,
        direction: MediaDirection,
    ) -> Result<MediaKeyContext, MediaSessionAllocatorError> {
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
        Ok(context)
    }
}

impl fmt::Debug for MediaSessionAllocator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MediaSessionAllocator")
            .field("root", &"[REDACTED]")
            .field("issued_len", &self.issued_len)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use sonimesh_protocol::{MediaDirection, SessionSecret};

    use super::{MAX_RETAINED_MEDIA_IDENTITIES, MediaSessionAllocator, MediaSessionAllocatorError};
    use crate::MediaStreamIdentity;

    const DIRECTION: MediaDirection = MediaDirection::AndroidToDesktop;

    const fn identity(session_id: u64, stream_id: u32, key_epoch: u16) -> MediaStreamIdentity {
        MediaStreamIdentity {
            session_id,
            stream_id,
            key_epoch,
        }
    }

    #[test]
    fn issues_a_fresh_session_for_the_first_authenticated_identity() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        let id = identity(7, 1, 0);

        let context = allocator.supply(id, DIRECTION).expect("fresh identity");

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

        let first = allocator.supply(identity(7, 1, 0), DIRECTION).unwrap();
        let rotated = allocator
            .supply(identity(7, 1, 1), DIRECTION)
            .expect("higher epoch is a fresh context");

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
            .expect("new session id is a fresh context");

        assert_eq!(fresh.session_id, 8);
    }

    #[test]
    fn direction_is_bound_into_the_no_reuse_context() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        allocator.supply(identity(7, 1, 0), DIRECTION).unwrap();

        let other_direction = allocator
            .supply(identity(7, 1, 0), MediaDirection::DesktopToAndroid)
            .expect("opposite direction is a distinct key context");

        assert_eq!(other_direction.direction, MediaDirection::DesktopToAndroid);
        assert_eq!(allocator.issued_count(), 2);
    }

    #[test]
    fn root_is_shared_across_fresh_sessions_and_redacted_in_debug() {
        let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
        let first = allocator.supply(identity(7, 1, 0), DIRECTION).unwrap();
        let second = allocator.supply(identity(8, 1, 0), DIRECTION).unwrap();

        let debug = format!("{allocator:?}");
        assert!(!debug.contains("A5"));
        assert!(debug.contains("issued_len: 2"));
        // The same retained root backs both fresh contexts.
        assert!(core::ptr::eq(allocator.root(), allocator.root()));
        let _ = (first, second);
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
