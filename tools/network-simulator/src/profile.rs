use std::fmt;

/// Hard bound on queued virtual packets, including duplicate copies.
pub const MAX_IN_FLIGHT_PACKETS: usize = 4_096;
/// Hard bound on base, jitter, and reorder delay combined.
pub const MAX_NETWORK_DELAY_MS: u64 = 60_000;

/// Deterministic network impairment configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkProfile {
    /// Independent random packet loss percentage.
    pub loss_percent: u8,
    /// Probability of scheduling one duplicate copy.
    pub duplicate_percent: u8,
    /// Probability of adding `reorder_delay_ms` to a packet.
    pub reorder_percent: u8,
    /// Fixed one-way delay before jitter.
    pub base_delay_ms: u64,
    /// Symmetric delay variation around the base delay.
    pub jitter_ms: u64,
    /// Extra delay used to create deterministic reordering.
    pub reorder_delay_ms: u64,
    /// Start one burst every N submitted packets; zero disables bursts.
    pub burst_interval_packets: u32,
    /// Consecutive packets dropped in each configured burst.
    pub burst_length_packets: u16,
    /// Hard bound on scheduled packet copies.
    pub max_in_flight_packets: usize,
    /// Deterministic pseudo-random seed.
    pub seed: u64,
}

impl NetworkProfile {
    /// Stable-WLAN starting point with no loss or duplication.
    #[must_use]
    pub const fn stable(seed: u64) -> Self {
        Self {
            loss_percent: 0,
            duplicate_percent: 0,
            reorder_percent: 0,
            base_delay_ms: 20,
            jitter_ms: 5,
            reorder_delay_ms: 30,
            burst_interval_packets: 0,
            burst_length_packets: 0,
            max_in_flight_packets: 256,
            seed,
        }
    }

    /// Validates all probability and resource bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ProfileError`] when a percentage, delay, burst, or queue bound
    /// is invalid.
    pub fn validate(self) -> Result<(), ProfileError> {
        if self.loss_percent > 100 || self.duplicate_percent > 100 || self.reorder_percent > 100 {
            return Err(ProfileError::InvalidPercentage);
        }
        if self.max_in_flight_packets == 0 {
            return Err(ProfileError::ZeroInFlightCapacity);
        }
        if self.max_in_flight_packets > MAX_IN_FLIGHT_PACKETS {
            return Err(ProfileError::InFlightCapacityTooLarge);
        }
        let total_delay = self
            .base_delay_ms
            .checked_add(self.jitter_ms)
            .and_then(|delay| delay.checked_add(self.reorder_delay_ms))
            .ok_or(ProfileError::DelayTooLarge)?;
        if total_delay > MAX_NETWORK_DELAY_MS {
            return Err(ProfileError::DelayTooLarge);
        }
        if self.burst_interval_packets == 0 && self.burst_length_packets != 0 {
            return Err(ProfileError::BurstIntervalMissing);
        }
        Ok(())
    }
}

impl Default for NetworkProfile {
    fn default() -> Self {
        Self::stable(0x5A17_2026)
    }
}

/// Invalid virtual-network profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfileError {
    /// A probability is greater than 100 percent.
    InvalidPercentage,
    /// In-flight queue must hold at least one packet.
    ZeroInFlightCapacity,
    /// In-flight queue exceeds [`MAX_IN_FLIGHT_PACKETS`].
    InFlightCapacityTooLarge,
    /// Combined network delay exceeds [`MAX_NETWORK_DELAY_MS`].
    DelayTooLarge,
    /// Burst length was set without a non-zero interval.
    BurstIntervalMissing,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid virtual-network profile: {self:?}")
    }
}

impl std::error::Error for ProfileError {}
