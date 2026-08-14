//! Deterministic, bounded network simulation for SoniMesh core regression tests.

mod clock;
mod control_scenario;
mod drift_scenario;
mod jitter_scheduler_scenario;
mod jitter_target_scenario;
mod mixer_controller_membership_scenario;
mod mixer_drift_scenario;
mod mixer_jitter_scheduler_scenario;
mod mixer_membership_scenario;
mod multi_stream_jitter_target_scenario;
mod multi_stream_scenario;
mod network;
mod opus_experiment;
mod profile;
mod scenario;

pub use clock::{ClockError, VirtualClock};
pub use control_scenario::{
    ControlScenarioError, ControlSimulationReport, FreshControlSessionReport,
    RetiredControlSessionReport, simulate_control_reconnect,
};
pub use drift_scenario::{
    DriftScenarioConfig, DriftScenarioError, DriftSimulationReport, simulate_drift,
};
pub use jitter_scheduler_scenario::{
    JitterSchedulerScenarioConfig, JitterSchedulerScenarioError, JitterSchedulerSimulationReport,
    simulate_jitter_scheduler,
};
pub use jitter_target_scenario::{
    JitterTargetScenarioConfig, JitterTargetScenarioError, JitterTargetSimulationReport,
    simulate_jitter_target,
};
pub use mixer_controller_membership_scenario::{
    DEFAULT_MIXER_CONTROLLER_PCM_BLOCK, DEFAULT_MIXER_CONTROLLER_SLOTS, MixerControllerGainScript,
    MixerControllerMixerReport, MixerControllerScenarioConfig, MixerControllerScenarioError,
    MixerControllerSimulationReport, MixerControllerStreamQuality, MixerControllerStreamReport,
    simulate_mixer_controller_membership,
};
pub use mixer_drift_scenario::{
    MAX_MIXER_DRIFT_STREAMS, MIN_MIXER_DRIFT_STREAMS, MixerDriftScenarioConfig,
    MixerDriftScenarioError, MixerDriftSimulationReport, MixerDriftStreamConfig,
    simulate_mixer_drift,
};
pub use mixer_jitter_scheduler_scenario::{
    DEFAULT_MIXER_JITTER_PCM_BLOCK, DEFAULT_MIXER_JITTER_SLOTS, MixerJitterGainScript,
    MixerJitterMixerReport, MixerJitterSchedulerScenarioConfig, MixerJitterSchedulerScenarioError,
    MixerJitterSchedulerSimulationReport, MixerJitterStreamQuality, StreamJitterSchedulerReport,
    simulate_mixer_jitter_scheduler,
};
pub use mixer_membership_scenario::{
    DEFAULT_MIXER_MEMBERSHIP_PCM_BLOCK, DEFAULT_MIXER_MEMBERSHIP_SLOTS, MixerMembershipMixerReport,
    MixerMembershipScenarioConfig, MixerMembershipScenarioError, MixerMembershipSimulationReport,
    simulate_mixer_membership,
};
pub use multi_stream_jitter_target_scenario::{
    MultiStreamJitterTargetScenarioConfig, MultiStreamJitterTargetScenarioError,
    MultiStreamJitterTargetSimulationReport, simulate_multi_stream_jitter_target,
};
pub use multi_stream_scenario::{
    MAX_MULTI_STREAM_STREAMS, MIN_MULTI_STREAM_STREAMS, MultiStreamScenarioConfig,
    MultiStreamScenarioError, MultiStreamSimulationReport, simulate_multi_stream,
};
pub use network::{Delivery, NetworkStats, SendOutcome, VirtualNetwork};
pub use opus_experiment::{
    OpusExperimentCase, OpusExperimentId, OpusWorkerExperimentReport, opus_experiment_cases,
    simulate_opus_experiment, simulate_opus_worker_experiment,
};
pub use profile::{
    MAX_BANDWIDTH_BYTES_PER_SECOND, MAX_IN_FLIGHT_PACKETS, MAX_NETWORK_DELAY_MS, NetworkProfile,
    ProfileError,
};
pub use scenario::{ScenarioConfig, ScenarioError, SimulationReport, simulate};
