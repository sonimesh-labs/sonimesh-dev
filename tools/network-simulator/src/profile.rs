use std::fmt;

/// Hard bound on queued virtual packets, including duplicate copies.
pub const MAX_IN_FLIGHT_PACKETS: usize = 4_096;
/// Hard bound on base, jitter, and reorder delay combined.
pub const MAX_NETWORK_DELAY_MS: u64 = 60_000;
/// Hard bound on one virtual-link bandwidth configuration.
pub const MAX_BANDWIDTH_BYTES_PER_SECOND: u64 = 1_000_000_000;
/// Hard bound on one sender-side socket-buffer configuration.
pub const MAX_SEND_BUFFER_BYTES: u64 = 64 * 1024 * 1024;

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
    /// Serial link bandwidth in bytes per second; zero disables shaping.
    pub bandwidth_bytes_per_second: u64,
    /// Sender-side socket buffer bound in bytes; `None` disables the model
    /// so the sender never reports backpressure.
    pub send_buffer_bytes: Option<u64>,
    /// Start one non-overlapping burst every N submitted packets; zero disables bursts.
    pub burst_interval_packets: u32,
    /// Consecutive packets dropped in each configured non-overlapping burst.
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
            bandwidth_bytes_per_second: 0,
            send_buffer_bytes: None,
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
        if self.bandwidth_bytes_per_second > MAX_BANDWIDTH_BYTES_PER_SECOND {
            return Err(ProfileError::BandwidthTooLarge);
        }
        if self.burst_interval_packets == 0 && self.burst_length_packets != 0 {
            return Err(ProfileError::BurstIntervalMissing);
        }
        if u32::from(self.burst_length_packets) > self.burst_interval_packets {
            return Err(ProfileError::BurstLengthExceedsInterval);
        }
        if let Some(bytes) = self.send_buffer_bytes {
            if bytes == 0 {
                return Err(ProfileError::ZeroSendBuffer);
            }
            if bytes > MAX_SEND_BUFFER_BYTES {
                return Err(ProfileError::SendBufferTooLarge);
            }
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
    /// Burst length would overlap the next configured burst interval.
    BurstLengthExceedsInterval,
    /// Link bandwidth exceeds [`MAX_BANDWIDTH_BYTES_PER_SECOND`].
    BandwidthTooLarge,
    /// Sender-side socket buffer must hold at least one byte.
    ZeroSendBuffer,
    /// Sender-side socket buffer exceeds [`MAX_SEND_BUFFER_BYTES`].
    SendBufferTooLarge,
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPercentage => {
                formatter.write_str("virtual-network impairment percentage exceeds 100")
            }
            Self::ZeroInFlightCapacity => {
                formatter.write_str("virtual-network in-flight capacity must be at least one")
            }
            Self::InFlightCapacityTooLarge => formatter
                .write_str("virtual-network in-flight capacity exceeds the explicit hard bound"),
            Self::DelayTooLarge => formatter
                .write_str("virtual-network combined delay exceeds the explicit hard bound"),
            Self::BurstIntervalMissing => formatter
                .write_str("virtual-network burst length requires a non-zero burst interval"),
            Self::BurstLengthExceedsInterval => {
                formatter.write_str("virtual-network burst length must not exceed its interval")
            }
            Self::BandwidthTooLarge => {
                formatter.write_str("virtual-network bandwidth exceeds the explicit hard bound")
            }
            Self::ZeroSendBuffer => {
                formatter.write_str("virtual-network send buffer must hold at least one byte")
            }
            Self::SendBufferTooLarge => {
                formatter.write_str("virtual-network send buffer exceeds the explicit hard bound")
            }
        }
    }
}

impl std::error::Error for ProfileError {}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn accepts_disabled_bursts() {
        let profile = NetworkProfile::stable(1);

        assert_eq!(profile.validate(), Ok(()));
    }

    #[test]
    fn rejects_burst_length_without_an_interval_before_overlap_validation() {
        let mut profile = NetworkProfile::stable(1);
        profile.burst_length_packets = 1;

        assert_eq!(profile.validate(), Err(ProfileError::BurstIntervalMissing));
    }

    #[test]
    fn allows_adjacent_non_overlapping_bursts() {
        let mut profile = NetworkProfile::stable(1);
        profile.burst_interval_packets = 4;
        profile.burst_length_packets = 4;

        assert_eq!(profile.validate(), Ok(()));
    }

    #[test]
    fn rejects_overlapping_burst_configuration() {
        let mut profile = NetworkProfile::stable(1);
        profile.burst_interval_packets = 3;
        profile.burst_length_packets = 4;

        assert_eq!(
            profile.validate(),
            Err(ProfileError::BurstLengthExceedsInterval)
        );
    }

    #[test]
    fn rejects_an_unbounded_bandwidth_configuration() {
        let mut profile = NetworkProfile::stable(1);
        profile.bandwidth_bytes_per_second = MAX_BANDWIDTH_BYTES_PER_SECOND + 1;

        assert_eq!(profile.validate(), Err(ProfileError::BandwidthTooLarge));
    }

    #[test]
    fn profile_errors_have_descriptive_stable_messages() {
        assert_eq!(
            ProfileError::InvalidPercentage.to_string(),
            "virtual-network impairment percentage exceeds 100"
        );
        assert_eq!(
            ProfileError::ZeroInFlightCapacity.to_string(),
            "virtual-network in-flight capacity must be at least one"
        );
        assert_eq!(
            ProfileError::InFlightCapacityTooLarge.to_string(),
            "virtual-network in-flight capacity exceeds the explicit hard bound"
        );
        assert_eq!(
            ProfileError::DelayTooLarge.to_string(),
            "virtual-network combined delay exceeds the explicit hard bound"
        );
        assert_eq!(
            ProfileError::BurstIntervalMissing.to_string(),
            "virtual-network burst length requires a non-zero burst interval"
        );
        assert_eq!(
            ProfileError::BurstLengthExceedsInterval.to_string(),
            "virtual-network burst length must not exceed its interval"
        );
        assert_eq!(
            ProfileError::BandwidthTooLarge.to_string(),
            "virtual-network bandwidth exceeds the explicit hard bound"
        );
        assert!(Error::source(&ProfileError::InvalidPercentage).is_none());
    }

    #[test]
    fn rejects_a_zero_send_buffer() {
        let mut profile = NetworkProfile::stable(1);
        profile.send_buffer_bytes = Some(0);

        assert_eq!(profile.validate(), Err(ProfileError::ZeroSendBuffer));
    }

    #[test]
    fn rejects_an_unbounded_send_buffer_configuration() {
        let mut profile = NetworkProfile::stable(1);
        profile.send_buffer_bytes = Some(MAX_SEND_BUFFER_BYTES + 1);

        assert_eq!(profile.validate(), Err(ProfileError::SendBufferTooLarge));
    }

    #[test]
    fn accepts_a_bounded_send_buffer() {
        let mut profile = NetworkProfile::stable(1);
        profile.send_buffer_bytes = Some(1_024);

        assert_eq!(profile.validate(), Ok(()));
    }

    #[test]
    fn send_buffer_errors_have_descriptive_stable_messages() {
        assert_eq!(
            ProfileError::ZeroSendBuffer.to_string(),
            "virtual-network send buffer must hold at least one byte"
        );
        assert_eq!(
            ProfileError::SendBufferTooLarge.to_string(),
            "virtual-network send buffer exceeds the explicit hard bound"
        );
    }
}
