//! Deterministic property coverage for the media-session allocator.
//!
//! `MediaSessionAllocator` (ADR 0017) issues a fresh media key context only for
//! an identity the authenticated control plane supplies whose full key context
//! (session, stream, epoch, direction) was never issued before in the pairing
//! lifetime, and fails closed when its bounded no-reuse record is full. This
//! test drives randomized identity/direction supply scripts against an
//! independent append-only set model and asserts the exact outcome after every
//! step: the allocator never re-issues a context, never exceeds the bound,
//! never records a failed supply, and its `contains` view always agrees with
//! the model.

use sonimesh_app::{
    MAX_RETAINED_MEDIA_IDENTITIES, MediaSessionAllocator, MediaSessionAllocatorError,
    MediaStreamIdentity,
};
use sonimesh_protocol::{MediaDirection, MediaKeyContext, SessionSecret};

const DIRECTIONS: [MediaDirection; 2] = [
    MediaDirection::AndroidToDesktop,
    MediaDirection::DesktopToAndroid,
];

/// Small deterministic xorshift64 generator; no external dependency.
struct XorShift64(u64);

impl XorShift64 {
    fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    fn next_usize(&mut self, bound: usize) -> usize {
        if bound == 0 {
            return 0;
        }
        self.next_u32() as usize % bound
    }
}

/// Independent append-only set model of the no-reuse record. It derives every
/// expected outcome from its own state rather than the allocator's field
/// layout, so the two cannot share a structural bug by construction.
struct IssuedModel {
    issued: Vec<MediaKeyContext>,
}

impl IssuedModel {
    fn new() -> Self {
        Self { issued: Vec::new() }
    }

    fn expected(
        &self,
        identity: MediaStreamIdentity,
        direction: MediaDirection,
    ) -> Result<MediaKeyContext, MediaSessionAllocatorError> {
        let Ok(context) = identity.media_key_context(direction) else {
            return Err(MediaSessionAllocatorError::InvalidIdentity);
        };
        if self.issued.contains(&context) {
            return Err(MediaSessionAllocatorError::IdentityReused);
        }
        if self.issued.len() == MAX_RETAINED_MEDIA_IDENTITIES {
            return Err(MediaSessionAllocatorError::IdentityCapacityExceeded);
        }
        Ok(context)
    }

    fn record(&mut self, context: MediaKeyContext) {
        self.issued.push(context);
    }
}

const fn identity(session_id: u64, stream_id: u32, key_epoch: u16) -> MediaStreamIdentity {
    MediaStreamIdentity {
        session_id,
        stream_id,
        key_epoch,
    }
}

/// Branch counters accumulated across the seeded walks.
#[derive(Default)]
struct WalkCounters {
    fresh: usize,
    invalid: usize,
    reused: usize,
    capacity: usize,
    directions: [bool; 2],
}

/// One deterministic walk over a fresh allocator.
struct WalkConfig {
    seed: u64,
    iterations: usize,
    /// Narrow identity space forces reuse and capacity fill.
    narrow: bool,
    /// Probability denominator for replaying an already-issued context.
    replay_frequency: u32,
    /// Probability denominator for injecting a zero identifier.
    zero_frequency: u32,
}

fn random_identity(rng: &mut XorShift64, narrow: bool) -> MediaStreamIdentity {
    if narrow {
        let session_id = u64::try_from(rng.next_usize(3)).expect("small bound") + 1;
        let stream_id = u32::try_from(rng.next_usize(4)).expect("small bound") + 1;
        let key_epoch = u16::try_from(rng.next_usize(3)).expect("small bound");
        identity(session_id, stream_id, key_epoch)
    } else {
        let session_id = u64::from(rng.next_u32()) | (u64::from(rng.next_u32()) << 32);
        let stream_id = rng.next_u32();
        let key_epoch = u16::try_from(rng.next_usize(u16::MAX as usize + 1)).expect("u16 bound");
        identity(session_id, stream_id, key_epoch)
    }
}

fn run_walk(config: &WalkConfig, counters: &mut WalkCounters) {
    let mut rng = XorShift64(config.seed);
    let mut allocator = MediaSessionAllocator::new(SessionSecret::new([0xA5; 32]));
    let mut model = IssuedModel::new();

    for _ in 0..config.iterations {
        let direction = DIRECTIONS[rng.next_usize(DIRECTIONS.len())];

        // Occasionally replay an already-issued context so the reuse branch is
        // exercised without depending on random collisions.
        let id = if config.replay_frequency != 0
            && rng.next_u32() % config.replay_frequency == 0
            && !model.issued.is_empty()
        {
            let context = model.issued[rng.next_usize(model.issued.len())];
            identity(context.session_id, context.stream_id, context.key_epoch)
        } else if config.zero_frequency != 0 && rng.next_u32() % config.zero_frequency == 0 {
            let zero_session = rng.next_u32() % 2 == 0;
            if zero_session {
                identity(0, rng.next_u32() | 1, 0)
            } else {
                identity(u64::from(rng.next_u32()) | 1, 0, 0)
            }
        } else {
            random_identity(&mut rng, config.narrow)
        };

        let real = allocator.supply(id, direction).map(|grant| grant.context());
        let expected = model.expected(id, direction);
        assert_eq!(
            real, expected,
            "supply diverged from the no-reuse model for {id:?} {direction:?}"
        );
        match real {
            Ok(context) => {
                counters.fresh += 1;
                model.record(context);
            }
            Err(MediaSessionAllocatorError::InvalidIdentity) => counters.invalid += 1,
            Err(MediaSessionAllocatorError::IdentityReused) => counters.reused += 1,
            Err(MediaSessionAllocatorError::IdentityCapacityExceeded) => counters.capacity += 1,
        }

        // The allocator's record must exactly mirror the model after every
        // step: nothing lost, nothing phantom, and the count in lock-step.
        assert_eq!(
            allocator.issued_count(),
            model.issued.len(),
            "issued count drifted from the model"
        );
        assert!(
            model
                .issued
                .iter()
                .all(|issued| allocator.contains(*issued)),
            "an issued context vanished from the allocator record"
        );
        if let Ok(context) = expected {
            assert_eq!(
                allocator.contains(context),
                model.issued.contains(&context),
                "contains disagrees with the model"
            );
        }
        counters.directions[direction as usize] = true;
    }
}

/// `model` and `allocator` are short local names, not a design smell, so the
/// noisy `similar_names` lint is allowed here.
#[allow(clippy::similar_names)]
#[test]
fn seeded_supply_walks_match_an_independent_no_reuse_model() {
    let mut counters = WalkCounters::default();

    let configs = [
        // Narrow identity space: forces reuse and capacity fill quickly.
        WalkConfig {
            seed: 0x5EED_0011,
            iterations: 40_000,
            narrow: true,
            replay_frequency: 7,
            zero_frequency: 31,
        },
        // Wide identity space: mostly fresh issues until the bound fills.
        WalkConfig {
            seed: 0x5EED_0012,
            iterations: 40_000,
            narrow: false,
            replay_frequency: 0,
            zero_frequency: 0,
        },
        // Balanced walk with heavy replay and zero-id injection.
        WalkConfig {
            seed: 0x5EED_0013,
            iterations: 40_000,
            narrow: true,
            replay_frequency: 3,
            zero_frequency: 11,
        },
    ];

    for config in &configs {
        run_walk(config, &mut counters);
    }

    // Every branch and direction was exercised across the walks.
    assert_ne!(counters.fresh, 0, "fresh issue branch never hit");
    assert_ne!(counters.invalid, 0, "zero-identifier branch never hit");
    assert_ne!(counters.reused, 0, "reuse rejection branch never hit");
    assert_ne!(counters.capacity, 0, "capacity-fill branch never hit");
    assert!(
        counters.directions[0] && counters.directions[1],
        "both media directions were not exercised"
    );
}
