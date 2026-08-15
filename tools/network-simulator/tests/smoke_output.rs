//! Regression coverage for the deterministic smoke runner's printed report.
//!
//! CI gates `cargo run -p sonimesh-network-simulator`, so this test pins what
//! that gate actually prints: the seeded run is byte-for-byte deterministic,
//! every data line parses into `key=value` fields, and the sections that
//! encode `SimulationReport` accounting satisfy their cross-field invariants
//! (playout plus PLC equals the frame count, no loss means no concealment,
//! and each lost base-profile frame conceals exactly once). This pins the
//! counter consistency that previously drifted between report sections.

use std::collections::BTreeMap;
use std::process::Command;

/// Default scenario frame count used by every smoke section.
const FRAMES: u64 = 500;

/// Hard-deadline slack for the `mixer_jitter_scheduler` smoke profiles,
/// mirroring the walk/fuzz formula
/// `frames * (max_gate_ticks + 2) * frame_duration_ms + base_delay_ms +
/// jitter_ms + reorder_delay_ms + 2`. Both smoke profiles pin `frames=200`
/// and keep the remaining scenario defaults (`max_gate_ticks=8`,
/// `frame_duration_ms=20`, and the stable network profile's
/// `base_delay_ms=20`, `jitter_ms=5`, `reorder_delay_ms=30`), so the bound
/// is `200 * 10 * 20 + 20 + 5 + 30 + 2`.
const MIXER_HARD_DEADLINE_MS: u64 = 200 * (8 + 2) * 20 + 20 + 5 + 30 + 2;

/// Ordered smoke section headers; pinning the order documents the suite.
const HEADERS: &[&str] = &[
    "SoniMesh deterministic virtual-network profiles",
    "SoniMesh shaped finite-link and sender-side socket-buffer profile",
    "SoniMesh codec-neutral Opus experiment suite",
    "SoniMesh codec-neutral mock decode-worker suite",
    "SoniMesh multi-stream shared-link suite",
    "SoniMesh fixed-slot mixer membership suite",
    "SoniMesh multi-stream mixer-drift suite",
    "SoniMesh controller-driven mixer membership suite",
    "SoniMesh combined jitter-scheduler and mixer suite",
    "SoniMesh adaptive jitter-target suite",
    "SoniMesh multi-stream adaptive jitter-target suite",
    "SoniMesh target-gated playout scheduler suite",
];

/// Runs the smoke binary and returns its stdout, asserting a clean exit.
fn run_smoke() -> String {
    let binary = env!("CARGO_BIN_EXE_sonimesh-network-simulator");
    let output = Command::new(binary)
        .output()
        .expect("smoke runner binary executes");
    assert!(
        output.status.success(),
        "smoke runner exited nonzero: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "smoke runner must not write to stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("smoke runner prints utf-8 stdout")
}

/// Parses one `key=value` data line into a normalized field map.
///
/// Tolerates the `loss= 0%` spacing quirk and parenthesized groups such as
/// `(min=2 base=3 max=4)`: keys keep only ASCII alphanumerics and `_`, and
/// numeric values are read through the typed accessors below.
fn fields(line: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut pending: Option<String> = None;
    for token in line.split_whitespace() {
        if let Some((key, value)) = token.split_once('=') {
            if value.is_empty() {
                pending = Some(key.to_string());
            } else {
                map.insert(normalize_key(key), value.to_string());
                pending = None;
            }
        } else if let Some(key) = pending.take() {
            map.insert(normalize_key(&key), token.to_string());
        }
    }
    map
}

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// Reads a nonnegative integer field, ignoring any trailing unit suffix.
fn number(map: &BTreeMap<String, String>, key: &str) -> u64 {
    let value = map
        .get(key)
        .unwrap_or_else(|| panic!("missing {key} field"));
    let digits: String = value.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("{key} is not a nonnegative integer: {value:?}"))
}

/// Reads a signed integer field, ignoring any trailing unit suffix.
fn signed(map: &BTreeMap<String, String>, key: &str) -> i64 {
    let value = map
        .get(key)
        .unwrap_or_else(|| panic!("missing {key} field"));
    let (sign, rest) = value
        .strip_prefix('-')
        .map_or((1_i64, value.as_str()), |rest| (-1_i64, rest));
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    sign * digits
        .parse::<i64>()
        .unwrap_or_else(|_| panic!("{key} is not a signed integer: {value:?}"))
}

#[test]
#[allow(clippy::too_many_lines)]
fn smoke_output_is_deterministic_and_reports_consistent_accounting() {
    let first = run_smoke();
    let second = run_smoke();
    assert_eq!(
        first, second,
        "seeded smoke output is byte-for-byte deterministic"
    );

    let lines: Vec<&str> = first.lines().collect();
    let headers: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("SoniMesh "))
        .collect();
    assert_eq!(
        headers, HEADERS,
        "smoke sections appear in the documented order"
    );

    for line in lines.iter().filter(|line| !line.starts_with("SoniMesh ")) {
        assert!(
            line.contains('='),
            "data line has key=value fields: {line:?}"
        );
    }

    // Base profiles: four loss levels over the default 500-frame scenario.
    let base: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("loss="))
        .collect();
    assert_eq!(base.len(), 4, "one line per configured loss level");
    for line in &base {
        let parsed = fields(line);
        assert_eq!(number(&parsed, "sent"), FRAMES, "{line}");
        assert_eq!(
            number(&parsed, "decoded") + number(&parsed, "plc"),
            FRAMES,
            "{line}"
        );
        let loss = number(&parsed, "loss");
        let network_lost = number(&parsed, "network_lost");
        if loss == 0 {
            assert_eq!(network_lost, 0, "{line}");
            assert_eq!(number(&parsed, "plc"), 0, "{line}");
        } else {
            assert!(number(&parsed, "plc") > 0, "loss conceals with PLC: {line}");
        }
        assert_eq!(
            number(&parsed, "late"),
            0,
            "the 60 ms target absorbs jitter and reorder: {line}"
        );
        assert_eq!(
            network_lost,
            number(&parsed, "plc"),
            "no late arrivals: every lost frame conceals once: {line}"
        );
    }

    // Shaped profile: the finite link plus a bounded sender-side socket
    // buffer exercises backpressure on the CI smoke gate, and every admitted
    // frame is either serialized or concealed.
    let shaped: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("shaped "))
        .collect();
    assert_eq!(shaped.len(), 1, "one shaped profile line is printed");
    let shaped_line = shaped[0];
    let parsed = fields(shaped_line);
    let sent = number(&parsed, "sent");
    let full = number(&parsed, "full");
    let scheduled = number(&parsed, "scheduled");
    let queued = number(&parsed, "queued_bytes");
    let serialized = number(&parsed, "serialized_bytes");
    assert_eq!(sent, 200, "{shaped_line}");
    assert_eq!(
        number(&parsed, "decoded") + number(&parsed, "plc"),
        sent,
        "{shaped_line}"
    );
    assert!(
        full > 0,
        "the shaped profile must exercise backpressure: {shaped_line}"
    );
    assert_eq!(
        full + scheduled,
        sent,
        "no loss or duplication: every submission is admitted or backpressured: {shaped_line}"
    );
    assert_eq!(
        queued, 0,
        "the final snapshot drains the bounded socket buffer: {shaped_line}"
    );
    assert_eq!(
        serialized,
        scheduled * 256,
        "serialized bytes equal one 256-byte datagram per scheduled copy: {shaped_line}"
    );

    // Opus experiment suite: every case still accounts all 500 frames, and a
    // case with no network loss conceals nothing.
    let opus: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("opus="))
        .collect();
    assert_eq!(opus.len(), 8, "the A-F matrix prints every variant");
    for line in &opus {
        let parsed = fields(line);
        assert_eq!(
            number(&parsed, "decoded") + number(&parsed, "plc"),
            FRAMES,
            "{line}"
        );
        if number(&parsed, "network_lost") == 0 {
            assert_eq!(number(&parsed, "plc"), 0, "{line}");
        }
    }

    // Mock decode-worker suite mirrors the opus decoded/PLC split.
    let mock: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("mock_worker"))
        .collect();
    assert_eq!(mock.len(), 8, "the A-F matrix prints every mock variant");
    for line in &mock {
        let parsed = fields(line);
        assert_eq!(
            number(&parsed, "decoded") + number(&parsed, "plc"),
            FRAMES,
            "{line}"
        );
    }

    // Adaptive jitter targets: every published final target stays inside its
    // configured min/max water levels.
    let target_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| {
            line.starts_with("jitter_target")
                || line.starts_with("multi_stream_jitter_target stream=")
                || line.starts_with("scheduler")
        })
        .collect();
    assert!(!target_lines.is_empty(), "target water levels are printed");
    for line in &target_lines {
        let parsed = fields(line);
        let min = number(&parsed, "min");
        let max = number(&parsed, "max");
        let target = number(&parsed, "final_target");
        assert!(
            min <= target && target <= max,
            "target within water levels: {line}"
        );
    }

    // Mixer drift: every hard resync is exactly one low- or high-side reset
    // and the published adjustment never exceeds the configured ±500 ppm cap.
    let drift_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| line.starts_with("mixer_drift"))
        .collect();
    assert!(!drift_lines.is_empty(), "mixer drift lines are printed");
    for line in &drift_lines {
        let parsed = fields(line);
        assert_eq!(
            signed(&parsed, "low") + signed(&parsed, "high"),
            signed(&parsed, "resyncs"),
            "{line}"
        );
        assert!(
            signed(&parsed, "final_adjustment").unsigned_abs() <= 500,
            "adjustment stays within the ppm cap: {line}"
        );
    }

    // Mixer suites: active streams never exceed the fixed slot bound and the
    // rendered bus stays finite.
    let mixer_lines: Vec<&str> = lines
        .iter()
        .copied()
        .filter(|line| {
            line.starts_with("mixer_membership")
                || line.starts_with("mixer_jitter_scheduler")
                || line.starts_with("mixer_controller_membership")
        })
        .collect();
    assert!(!mixer_lines.is_empty(), "mixer suite lines are printed");
    for line in &mixer_lines {
        let parsed = fields(line);
        assert!(
            number(&parsed, "active") <= number(&parsed, "slots"),
            "{line}"
        );
        assert_eq!(
            parsed.get("finite").map(String::as_str),
            Some("true"),
            "{line}"
        );
        if parsed.get("projection_matched").map(String::as_str) == Some("true") {
            assert_eq!(
                number(&parsed, "active"),
                number(&parsed, "projected"),
                "projection matches the real slot table: {line}"
            );
        }
        if line.contains("stream0_plc_ratio") {
            assert!(
                number(&parsed, "stream0_plc_ratio") <= 1_000,
                "PLC ratio stays within milli-unit bound: {line}"
            );
        }
        if line.contains("stream1_plc_ratio") {
            assert!(
                number(&parsed, "stream1_plc_ratio") <= 1_000,
                "PLC ratio stays within milli-unit bound: {line}"
            );
        }
        if line.contains("stream0_forced") {
            assert!(
                number(&parsed, "stream0_forced") <= number(&parsed, "submitted"),
                "forced playouts stay within the frame count: {line}"
            );
        }
        if line.contains("stream1_forced") {
            assert!(
                number(&parsed, "stream1_forced") <= number(&parsed, "submitted"),
                "forced playouts stay within the frame count: {line}"
            );
        }
        if line.starts_with("mixer_jitter_scheduler streams=4") {
            assert!(
                number(&parsed, "forced") <= number(&parsed, "submitted"),
                "forced playouts stay within the frame count: {line}"
            );
        }
        if line.starts_with("mixer_jitter_scheduler") {
            assert!(
                number(&parsed, "excess_latency") <= MIXER_HARD_DEADLINE_MS,
                "completion must never exceed the hard deadline: {line}"
            );
        }
    }
}
