//! Deterministic, bounded network simulation for SoniMesh core regression tests.

mod clock;
mod drift_scenario;
mod network;
mod profile;
mod scenario;

pub use clock::{ClockError, VirtualClock};
pub use drift_scenario::{
    DriftScenarioConfig, DriftScenarioError, DriftSimulationReport, simulate_drift,
};
pub use network::{Delivery, NetworkStats, SendOutcome, VirtualNetwork};
pub use profile::{MAX_IN_FLIGHT_PACKETS, MAX_NETWORK_DELAY_MS, NetworkProfile, ProfileError};
pub use scenario::{ScenarioConfig, ScenarioError, SimulationReport, simulate};
