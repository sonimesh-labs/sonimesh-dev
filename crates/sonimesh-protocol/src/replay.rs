/// Width of the authenticated replay window.
const WINDOW_BITS: u32 = 64;

/// Result of recording an already-authenticated sequence number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayDecision {
    /// The sequence has not appeared in the active window.
    Accepted,
    /// The sequence is already present in the active window.
    Duplicate,
    /// The sequence precedes the active replay window.
    TooOld,
}

/// Fixed-size replay window for one stream and key epoch.
///
/// Call [`Self::accept_authenticated`] only after AEAD authentication succeeds.
/// A sequence wrap requires a new key epoch and a fresh window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReplayWindow {
    highest: Option<u32>,
    seen: u64,
}

impl ReplayWindow {
    /// Creates an empty replay window.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            highest: None,
            seen: 0,
        }
    }

    /// Records a sequence only after the packet has authenticated successfully.
    pub fn accept_authenticated(&mut self, sequence: u32) -> ReplayDecision {
        let Some(highest) = self.highest else {
            self.highest = Some(sequence);
            self.seen = 1;
            return ReplayDecision::Accepted;
        };

        if sequence > highest {
            let shift = sequence - highest;
            self.seen = if shift >= WINDOW_BITS {
                1
            } else {
                (self.seen << shift) | 1
            };
            self.highest = Some(sequence);
            return ReplayDecision::Accepted;
        }

        let distance = highest - sequence;
        if distance >= WINDOW_BITS {
            return ReplayDecision::TooOld;
        }
        let bit = 1_u64 << distance;
        if self.seen & bit != 0 {
            ReplayDecision::Duplicate
        } else {
            self.seen |= bit;
            ReplayDecision::Accepted
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_reordering_once() {
        let mut window = ReplayWindow::new();
        assert_eq!(window.accept_authenticated(10), ReplayDecision::Accepted);
        assert_eq!(window.accept_authenticated(12), ReplayDecision::Accepted);
        assert_eq!(window.accept_authenticated(11), ReplayDecision::Accepted);
        assert_eq!(window.accept_authenticated(11), ReplayDecision::Duplicate);
    }

    #[test]
    fn rejects_values_outside_window() {
        let mut window = ReplayWindow::new();
        assert_eq!(window.accept_authenticated(1), ReplayDecision::Accepted);
        assert_eq!(window.accept_authenticated(65), ReplayDecision::Accepted);
        assert_eq!(window.accept_authenticated(1), ReplayDecision::TooOld);
    }

    #[test]
    fn large_jump_resets_bitmap_without_allocating() {
        let mut window = ReplayWindow::new();
        assert_eq!(window.accept_authenticated(2), ReplayDecision::Accepted);
        assert_eq!(window.accept_authenticated(1000), ReplayDecision::Accepted);
        assert_eq!(window.accept_authenticated(999), ReplayDecision::Accepted);
    }

    #[test]
    fn keeps_the_oldest_sequence_in_window_after_a_maximum_in_window_shift() {
        let mut window = ReplayWindow::new();
        assert_eq!(window.accept_authenticated(1), ReplayDecision::Accepted);
        assert_eq!(
            window.accept_authenticated(1 + (WINDOW_BITS - 1)),
            ReplayDecision::Accepted
        );
        // The previous highest is still inside the window (distance 63).
        assert_eq!(window.accept_authenticated(1), ReplayDecision::Duplicate);
        // One more advance pushes it out (distance 64).
        assert_eq!(
            window.accept_authenticated(1 + WINDOW_BITS),
            ReplayDecision::Accepted
        );
        assert_eq!(window.accept_authenticated(1), ReplayDecision::TooOld);
    }

    #[test]
    fn accepts_the_oldest_in_window_sequence_at_the_maximum_value() {
        let mut window = ReplayWindow::new();
        assert_eq!(
            window.accept_authenticated(u32::MAX),
            ReplayDecision::Accepted
        );
        assert_eq!(
            window.accept_authenticated(u32::MAX - (WINDOW_BITS - 1)),
            ReplayDecision::Accepted
        );
        assert_eq!(
            window.accept_authenticated(u32::MAX - WINDOW_BITS),
            ReplayDecision::TooOld
        );
        assert_eq!(
            window.accept_authenticated(u32::MAX - (WINDOW_BITS - 1)),
            ReplayDecision::Duplicate
        );
    }
}
