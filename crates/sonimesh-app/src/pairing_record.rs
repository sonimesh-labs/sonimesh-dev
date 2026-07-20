use core::fmt;

/// Largest permitted failed-proof count for one opaque pairing source.
pub const MAX_PAIRING_ATTEMPTS_PER_SOURCE: u8 = 16;

/// Opaque short-lived identifier for one desktop-created pairing invitation.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PairingId([u8; 16]);

impl PairingId {
    /// Creates an ID from 128 bits supplied by the platform random source.
    #[must_use]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for PairingId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingId([REDACTED])")
    }
}

/// Opaque per-record source token used for pairing-rate limits.
///
/// Callers must derive this from a short-lived, privacy-preserving transport
/// context. It is not a device identifier and must not contain a raw IP address.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct PairingSource([u8; 16]);

impl PairingSource {
    /// Creates an opaque source token supplied by the control transport.
    #[must_use]
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for PairingSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PairingSource([REDACTED])")
    }
}

/// Lifecycle state for one single-use pairing invitation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingRecordState {
    /// Invitation may accept a bounded proof attempt before its expiry.
    Active,
    /// A valid transcript-bound proof was accepted exactly once.
    Consumed,
    /// Invitation expired before a valid proof completed.
    Expired,
    /// Invitation or its trusted session was explicitly revoked.
    Revoked,
}

/// Result of recording one TLS-layer pairing proof verification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingAttemptOutcome {
    /// Proof was accepted and the invitation is now permanently consumed.
    Paired,
    /// Proof was invalid but this source still has bounded attempts remaining.
    Rejected {
        /// Failed attempts this source may still submit before rate limiting.
        attempts_remaining: u8,
    },
}

/// Failure while admitting a pairing proof result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairingRecordError {
    /// Compile-time source table capacity is zero.
    ZeroSourceCapacity,
    /// Per-source attempt limit is zero or exceeds the hard bound.
    InvalidAttemptLimit,
    /// Caller supplied a clock value earlier than a previous observation.
    TimeRegressed,
    /// Invitation already expired and no future proof may consume it.
    Expired,
    /// Invitation was consumed by an earlier valid proof.
    Consumed,
    /// Invitation or trusted session was explicitly revoked.
    Revoked,
    /// Source has exhausted its bounded proof attempts.
    SourceRateLimited,
    /// Fixed source table cannot admit a new distinct source.
    SourceTableFull,
}

impl fmt::Display for PairingRecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroSourceCapacity => {
                formatter.write_str("pairing source capacity must be non-zero")
            }
            Self::InvalidAttemptLimit => formatter.write_str("pairing attempt limit is invalid"),
            Self::TimeRegressed => formatter.write_str("pairing clock moved backward"),
            Self::Expired => formatter.write_str("pairing invitation expired"),
            Self::Consumed => formatter.write_str("pairing invitation was already consumed"),
            Self::Revoked => formatter.write_str("pairing invitation was revoked"),
            Self::SourceRateLimited => {
                formatter.write_str("pairing source exceeded its attempt limit")
            }
            Self::SourceTableFull => formatter.write_str("pairing source table is full"),
        }
    }
}

impl std::error::Error for PairingRecordError {}

#[derive(Clone, Copy)]
struct SourceAttempts {
    source: PairingSource,
    attempts: u8,
}

/// Bounded single-use pairing record owned by the desktop control domain.
///
/// This type is intentionally independent from QR serialization, TLS, secrets,
/// and OS credential stores. Those platform owners call [`Self::record_proof`]
/// only after a transcript-bound proof has been verified.
pub struct PairingRecord<const MAX_SOURCES: usize> {
    id: PairingId,
    expires_at_ms: u64,
    last_observed_ms: u64,
    max_attempts_per_source: u8,
    state: PairingRecordState,
    sources: [Option<SourceAttempts>; MAX_SOURCES],
}

impl<const MAX_SOURCES: usize> PairingRecord<MAX_SOURCES> {
    /// Creates an active invitation with a fixed source table and expiry time.
    ///
    /// `expires_at_ms` and future timestamps must use the same monotonic clock.
    ///
    /// # Errors
    ///
    /// Returns [`PairingRecordError`] for zero source capacity or an unbounded
    /// per-source proof-attempt configuration.
    pub fn new(
        id: PairingId,
        expires_at_ms: u64,
        max_attempts_per_source: u8,
    ) -> Result<Self, PairingRecordError> {
        if MAX_SOURCES == 0 {
            return Err(PairingRecordError::ZeroSourceCapacity);
        }
        if max_attempts_per_source == 0 || max_attempts_per_source > MAX_PAIRING_ATTEMPTS_PER_SOURCE
        {
            return Err(PairingRecordError::InvalidAttemptLimit);
        }
        Ok(Self {
            id,
            expires_at_ms,
            last_observed_ms: 0,
            max_attempts_per_source,
            state: PairingRecordState::Active,
            sources: [None; MAX_SOURCES],
        })
    }

    /// Returns the opaque invitation ID for the platform pairing owner.
    #[must_use]
    pub const fn id(&self) -> PairingId {
        self.id
    }

    /// Returns the current single-use lifecycle state.
    #[must_use]
    pub const fn state(&self) -> PairingRecordState {
        self.state
    }

    /// Observes time without admitting a pairing proof.
    ///
    /// This is suitable for low-rate control polling. A clock regression
    /// expires an active invitation so a later forward jump cannot revive it.
    ///
    /// # Errors
    ///
    /// Returns [`PairingRecordError::TimeRegressed`] after permanently expiring
    /// an active invitation when `now_ms` is earlier than a prior observation.
    pub fn observe(&mut self, now_ms: u64) -> Result<PairingRecordState, PairingRecordError> {
        if matches!(
            self.state,
            PairingRecordState::Consumed | PairingRecordState::Revoked
        ) {
            return Ok(self.state);
        }
        if now_ms < self.last_observed_ms {
            self.state = PairingRecordState::Expired;
            return Err(PairingRecordError::TimeRegressed);
        }

        self.last_observed_ms = now_ms;
        if self.state == PairingRecordState::Active && now_ms >= self.expires_at_ms {
            self.state = PairingRecordState::Expired;
        }
        Ok(self.state)
    }

    /// Permanently expires an active invitation without accepting a proof.
    ///
    /// A consumed record remains consumed to preserve the successful-pairing
    /// audit state for its control owner.
    pub fn expire(&mut self) {
        if self.state == PairingRecordState::Active {
            self.state = PairingRecordState::Expired;
        }
    }

    /// Permanently revokes this invitation or the trusted session it established.
    ///
    /// This is an in-memory lifecycle marker only. The platform pairing owner
    /// must separately remove durable trust, credential, and media material.
    pub fn revoke(&mut self) {
        self.state = PairingRecordState::Revoked;
    }

    /// Records the result of one externally verified transcript-bound proof.
    ///
    /// # Errors
    ///
    /// Returns a fail-closed error when the clock regresses, invitation expired
    /// or was consumed, the source rate limit was reached, or the fixed source
    /// table cannot admit another source.
    pub fn record_proof(
        &mut self,
        now_ms: u64,
        source: PairingSource,
        proof_verified: bool,
    ) -> Result<PairingAttemptOutcome, PairingRecordError> {
        match self.state {
            PairingRecordState::Consumed => return Err(PairingRecordError::Consumed),
            PairingRecordState::Revoked => return Err(PairingRecordError::Revoked),
            PairingRecordState::Active | PairingRecordState::Expired => {}
        }
        if self.observe(now_ms)? == PairingRecordState::Expired {
            return Err(PairingRecordError::Expired);
        }

        let max_attempts = self.max_attempts_per_source;
        let source_index = self.source_index(source)?;
        let Some(entry) = self.sources[source_index].as_mut() else {
            return Err(PairingRecordError::SourceTableFull);
        };
        if entry.attempts >= max_attempts {
            return Err(PairingRecordError::SourceRateLimited);
        }
        entry.attempts += 1;
        if proof_verified {
            self.state = PairingRecordState::Consumed;
            return Ok(PairingAttemptOutcome::Paired);
        }

        Ok(PairingAttemptOutcome::Rejected {
            attempts_remaining: max_attempts - entry.attempts,
        })
    }

    fn source_index(&mut self, source: PairingSource) -> Result<usize, PairingRecordError> {
        if let Some(index) = self
            .sources
            .iter()
            .position(|slot| slot.is_some_and(|existing| existing.source == source))
        {
            return Ok(index);
        }
        let Some(index) = self.sources.iter().position(Option::is_none) else {
            return Err(PairingRecordError::SourceTableFull);
        };
        self.sources[index] = Some(SourceAttempts {
            source,
            attempts: 0,
        });
        Ok(index)
    }
}
