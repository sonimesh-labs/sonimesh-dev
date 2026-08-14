#![no_main]

//! Fuzz the bounded reconnect-grace lifecycle state machine.
//!
//! The reconnect-grace owner decides how long a control session may stay
//! disconnected before it must expire. This target drives arbitrary
//! begin/observe/reconnected/cancel scripts with adversarial u64 clocks and
//! durations and compares every decision against an independent deadline
//! model, proving the controller cannot panic, cannot revive a force-expired
//! window, cannot report a stale or unsigned remaining time, and always
//! returns to idle after expiry, regression, reconnect, or cancel.

use libfuzzer_sys::fuzz_target;
use sonimesh_app::{
    ReconnectGraceController, ReconnectGraceError, ReconnectGraceObservation, ReconnectGraceState,
};

/// Reads one little-endian `u64` from `data`, wrapping around the slice.
fn next_u64(data: &[u8], offset: &mut usize) -> u64 {
    let mut bytes = [0_u8; 8];
    for byte in &mut bytes {
        *byte = data[*offset % data.len()];
        *offset += 1;
    }
    u64::from_le_bytes(bytes)
}

/// Controller plus an independent deadline-based model of the same window.
struct GraceModel {
    controller: ReconnectGraceController,
    active: bool,
    deadline_ms: u64,
    last_observed_ms: u64,
}

impl GraceModel {
    fn new() -> Self {
        Self {
            controller: ReconnectGraceController::new(),
            active: false,
            deadline_ms: 0,
            last_observed_ms: 0,
        }
    }

    fn state(&self) -> ReconnectGraceState {
        if self.active {
            ReconnectGraceState::InGrace
        } else {
            ReconnectGraceState::Idle
        }
    }

    fn begin(&mut self, now_ms: u64, duration_ms: u64) {
        let expected = if self.active {
            Err(ReconnectGraceError::AlreadyInGrace)
        } else if duration_ms == 0 {
            Err(ReconnectGraceError::ZeroDuration)
        } else if now_ms.checked_add(duration_ms).is_none() {
            Err(ReconnectGraceError::ExpiryOverflow)
        } else {
            self.active = true;
            self.deadline_ms = now_ms + duration_ms;
            self.last_observed_ms = now_ms;
            Ok(())
        };
        let actual = self.controller.begin(now_ms, duration_ms);
        assert_eq!(actual, expected, "begin diverged from the deadline model");
        assert_eq!(
            self.controller.state(),
            self.state(),
            "begin left the controller out of sync"
        );
    }

    fn observe(&mut self, now_ms: u64) {
        let expected = if !self.active {
            Ok(ReconnectGraceObservation::Idle)
        } else if now_ms < self.last_observed_ms {
            self.active = false;
            Err(ReconnectGraceError::TimeRegressed)
        } else {
            self.last_observed_ms = now_ms;
            if now_ms >= self.deadline_ms {
                self.active = false;
                Ok(ReconnectGraceObservation::Expired)
            } else {
                Ok(ReconnectGraceObservation::InGrace {
                    remaining_ms: self.deadline_ms - now_ms,
                })
            }
        };
        let actual = self.controller.observe(now_ms);
        assert_eq!(actual, expected, "observe diverged from the deadline model");
        assert_eq!(
            self.controller.state(),
            self.state(),
            "observe left the controller out of sync"
        );
    }

    fn reconnected(&mut self) {
        let was_active = self.active;
        self.active = false;
        assert_eq!(
            self.controller.reconnected(),
            was_active,
            "reconnected reported the wrong prior state"
        );
        assert_eq!(
            self.controller.state(),
            self.state(),
            "reconnected left the controller in grace"
        );
    }

    fn cancel(&mut self) {
        let was_active = self.active;
        self.active = false;
        assert_eq!(
            self.controller.cancel(),
            was_active,
            "cancel reported the wrong prior state"
        );
        assert_eq!(
            self.controller.state(),
            self.state(),
            "cancel left the controller in grace"
        );
    }
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() {
        return;
    }
    let mut model = GraceModel::new();

    // Deterministic edge coverage on every run: zero-clock arming, exact
    // deadline expiry, the u64::MAX deadline and overflow, zero duration,
    // clock regression without revival, and reconnect/cancel resets.
    model.begin(0, 1);
    model.observe(0);
    model.observe(1); // expires exactly at the deadline
    model.begin(u64::MAX - 1, 1);
    model.observe(u64::MAX); // expires at the u64::MAX deadline
    model.observe(u64::MAX); // stays idle
    model.begin(u64::MAX, 1); // deadline overflows
    model.begin(7, 0); // zero duration rejected
    model.begin(1_000, 5_000);
    model.observe(1_200);
    model.observe(900); // regression force-expires
    model.observe(2_000); // a forward jump cannot revive the window
    model.begin(100, 10);
    model.reconnected();
    model.begin(200, 10);
    model.cancel();

    // Fuzz-derived operations with adversarial clocks and durations.
    let mut offset = 0;
    let steps = 1 + data.len() % 64;
    for _ in 0..steps {
        let op = data[offset % data.len()];
        offset += 1;
        match op % 4 {
            0 => {
                let now_ms = next_u64(data, &mut offset);
                let duration_ms = next_u64(data, &mut offset);
                model.begin(now_ms, duration_ms);
            }
            1 => {
                let now_ms = next_u64(data, &mut offset);
                model.observe(now_ms);
            }
            2 => model.reconnected(),
            _ => model.cancel(),
        }
    }
});
