#![no_main]

//! Fuzz the bounded occupancy PI controller for per-stream PCM drift.
//!
//! Arbitrary validated controller parameters meet arbitrary occupancy
//! snapshots. The controller must never panic, must keep the requested ppm
//! adjustment inside the configured ratio cap, must move by at most the
//! configured per-update slew cap, and must retire all history on a hard
//! resync or an explicit reset. Arbitrary raw parameter sets must also run
//! through the validator without panicking.

use libfuzzer_sys::fuzz_target;
use sonimesh_audio::{
    DriftConfig, DriftController, DriftDecision, MAX_DRIFT_ADJUSTMENT_PPM,
    MAX_DRIFT_INTEGRAL_BLOCK_TICKS, MAX_PCM_QUEUE_BLOCKS,
};

fuzz_target!(|data: &[u8]| {
    if data.len() < 12 {
        return;
    }

    // Bounded fields chosen so `validate()` always accepts: the low threshold
    // is zero, the high threshold is the queue bound, and the target sits
    // strictly between them.
    let target_blocks =
        1 + u16::from_le_bytes([data[0], data[1]]) % (MAX_PCM_QUEUE_BLOCKS - 1) as u16;
    let proportional_ppm_per_block = 1 + i32::from(u16::from_le_bytes([data[2], data[3]])) % 500;
    let integral_ppm_per_block_tick = 1 + i32::from(u16::from_le_bytes([data[4], data[5]])) % 100;
    let max_adjustment_ppm =
        1 + i32::from(u16::from_le_bytes([data[6], data[7]])) % MAX_DRIFT_ADJUSTMENT_PPM;
    let max_step_ppm = 1 + i32::from(u16::from_le_bytes([data[8], data[9]])) % max_adjustment_ppm;
    let max_integral_block_ticks =
        1 + i32::from(u16::from_le_bytes([data[10], data[11]])) % MAX_DRIFT_INTEGRAL_BLOCK_TICKS;

    let config = DriftConfig {
        target_blocks,
        low_resync_blocks: 0,
        high_resync_blocks: MAX_PCM_QUEUE_BLOCKS as u16,
        proportional_ppm_per_block,
        integral_ppm_per_block_tick,
        max_adjustment_ppm,
        max_step_ppm,
        max_integral_block_ticks,
    };
    let mut controller = DriftController::new(config).expect("derived config is valid");

    // Arbitrary raw parameter sets must not panic validation; accepted ones
    // must construct a controller that survives one update.
    if data.len() >= 32 {
        let raw = DriftConfig {
            target_blocks: u16::from_le_bytes([data[16], data[17]]),
            low_resync_blocks: u16::from_le_bytes([data[18], data[19]]),
            high_resync_blocks: u16::from_le_bytes([data[20], data[21]]),
            proportional_ppm_per_block: i32::from_le_bytes([
                data[22], data[23], data[24], data[25],
            ]),
            integral_ppm_per_block_tick: i32::from_le_bytes([
                data[26], data[27], data[28], data[29],
            ]),
            max_adjustment_ppm: i32::from(data[30]),
            max_step_ppm: i32::from(data[31]),
            max_integral_block_ticks: 1,
        };
        if raw.validate().is_ok()
            && let Ok(mut raw_controller) = DriftController::new(raw)
        {
            let _ = raw_controller.update(u16::from_le_bytes([data[0], data[1]]));
        }
    }

    let max_adjustment = config.max_adjustment_ppm;
    let max_step = config.max_step_ppm;
    let mut previous = 0;
    for chunk in data.chunks(2) {
        if chunk.len() < 2 {
            break;
        }
        let occupancy = u16::from_le_bytes([chunk[0], chunk[1]]);
        match controller.update(occupancy) {
            DriftDecision::Adjust { ppm } => {
                assert!(ppm.abs() <= max_adjustment);
                assert!(
                    (ppm - previous).abs() <= max_step,
                    "slew {previous} -> {ppm} exceeds {max_step}"
                );
                assert_eq!(controller.adjustment_ppm(), ppm);
                previous = ppm;
            }
            DriftDecision::HardResync { .. } => {
                assert_eq!(controller.adjustment_ppm(), 0);
                previous = 0;
            }
        }
    }
    controller.reset();
    assert_eq!(controller.adjustment_ppm(), 0);
});
