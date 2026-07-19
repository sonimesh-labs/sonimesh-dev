//! Desktop control-domain coordination, independent of Tauri commands.

mod channel_mapped_render_source;
mod control_snapshot;
mod control_stream;
mod format_adapted_render_source_factory;
mod output_controller;
mod pairing_controller;
mod pairing_record;
mod pcm_render_source;
mod receiver_output_controller;
mod receiver_pcm_endpoint_factory;
mod receiver_pipeline;
mod sample_rate_mapped_render_source;
mod session;
mod stream_drift_controller;
mod udp_receiver_pipeline;

pub use channel_mapped_render_source::{ChannelMappedRenderSource, ChannelMappedRenderSourceError};
pub use control_snapshot::{DesktopControlSnapshot, OutputSnapshot, PairingSnapshotState};
pub use control_stream::{
    ControlOperationToken, ControlStreamAction, ControlStreamCoordinator, ControlStreamError,
    MediaStreamIdentity, StreamStartRequest,
};
pub use format_adapted_render_source_factory::{
    FormatAdaptedRenderSourceFactory, SourcePcmFormat, SourcePcmFormatError,
};
pub use output_controller::{OutputController, OutputLifecycleError, OutputSelection};
pub use pairing_controller::{PairingInvitationController, PairingInvitationError};
pub use pairing_record::{
    MAX_PAIRING_ATTEMPTS_PER_SOURCE, PairingAttemptOutcome, PairingId, PairingRecord,
    PairingRecordError, PairingRecordState, PairingSource,
};
pub use pcm_render_source::PcmRenderSource;
pub use receiver_output_controller::{
    ReceiverOutputController, ReceiverOutputLifecycleError, ReceiverPcmProducerInstaller,
};
pub use receiver_pcm_endpoint_factory::{
    ReceiverPcmEndpoint, ReceiverPcmEndpointError, ReceiverPcmEndpointFactory,
};
pub use receiver_pipeline::{
    FreshSessionInstallError, ReceiverPipelineError, ReceiverStreamPipeline,
};
pub use sample_rate_mapped_render_source::{
    DriftAdjustmentError, DriftAdjustmentHandle, SampleRateMappedRenderSource,
    SampleRateMappedRenderSourceError,
};
pub use session::{SessionEvent, SessionPhase, SessionState, SessionTransitionError};
pub use stream_drift_controller::StreamDriftController;
pub use udp_receiver_pipeline::{
    UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline, UdpReceiverSetupError,
};
