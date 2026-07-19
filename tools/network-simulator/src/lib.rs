//! Deterministic, bounded network simulation for SoniMesh core regression tests.

mod clock;
mod control_scenario;
mod drift_scenario;
mod network;
mod opus_experiment;
mod profile;
mod scenario;

pub use clock::{ClockError, VirtualClock};
pub use control_scenario::{
    ControlScenarioError, ControlSimulationReport, simulate_control_reconnect,
};
pub use drift_scenario::{
    DriftScenarioConfig, DriftScenarioError, DriftSimulationReport, simulate_drift,
};
pub use network::{Delivery, NetworkStats, SendOutcome, VirtualNetwork};
pub use opus_experiment::{
    OpusExperimentCase, OpusExperimentId, opus_experiment_cases, simulate_opus_experiment,
};
pub use profile::{
    MAX_BANDWIDTH_BYTES_PER_SECOND, MAX_IN_FLIGHT_PACKETS, MAX_NETWORK_DELAY_MS, NetworkProfile,
    ProfileError,
};
pub use scenario::{ScenarioConfig, ScenarioError, SimulationReport, simulate};
