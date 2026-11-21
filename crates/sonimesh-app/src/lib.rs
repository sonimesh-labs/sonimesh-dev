//! Desktop control-domain coordination, independent of Tauri commands.

mod arrival_clock;
mod channel_mapped_render_source;
mod control_listen;
mod control_snapshot;
mod control_stream;
mod control_to_mixer;
mod diagnostic_export_action;
mod export_store;
mod format_adapted_render_source_factory;
mod media_session_allocator;
mod mixer_membership_controller;
mod mixer_output_controller;
mod mixer_render_source;
mod mixer_stream_drift;
mod mixer_stream_session;
mod output_controller;
mod pairing_controller;
mod pairing_handshake;
mod pairing_record;
mod pcm_render_source;
mod receiver_output_controller;
mod receiver_pcm_endpoint_factory;
mod receiver_pipeline;
mod reconnect_grace;
mod sample_rate_mapped_render_source;
mod session;
mod stream_drift_controller;
mod stream_format_adapter;
mod udp_receiver_pipeline;

pub use arrival_clock::monotonic_tick_source;
pub use channel_mapped_render_source::{ChannelMappedRenderSource, ChannelMappedRenderSourceError};
pub use control_listen::{
    ControlListenError, DEFAULT_PAIRING_IO_TIMEOUT, InvitationInputs, TrustedControlChannel,
    accept_pairing_connection,
};
pub use control_snapshot::{
    DesktopControlSnapshot, OutputSnapshot, OutputSnapshotSource, PairingSnapshotState,
};
pub use control_stream::{
    ControlOperationToken, ControlReply, ControlSessionToken, ControlStreamAction,
    ControlStreamCoordinator, ControlStreamError, MAX_ACTIVE_STREAMS, MediaStreamIdentity,
    StreamStartRequest,
};
pub use control_to_mixer::{BridgeSnapshot, ControlToMixerBridge, ControlToMixerError};
pub use diagnostic_export_action::{
    DEFAULT_MAX_RETAINED_EXPORT_BYTES, DEFAULT_MAX_RETAINED_EXPORTS, DiagnosticExportAction,
    DiagnosticExportActionError, ExportReceipt, ExportRetentionJournal, ExportRetentionPolicy,
    IssuedDiagnosticExport,
};
pub use export_store::{ExportStore, ExportStoreError};
pub use format_adapted_render_source_factory::{
    FormatAdaptedRenderSourceFactory, SourcePcmFormat, SourcePcmFormatError,
};
pub use media_session_allocator::{
    IssuedMediaSession, MAX_RETAINED_MEDIA_IDENTITIES, MediaSessionAllocator,
    MediaSessionAllocatorError,
};
pub use mixer_membership_controller::{
    AdmittedAdaptedStream, AdmittedStream, MixerAdmissionError, MixerMembershipController,
};
pub use mixer_output_controller::{MixerOutputController, MixerOutputLifecycleError};
pub use mixer_render_source::{MixerRenderSource, MixerRenderSourceError};
pub use mixer_stream_drift::MixerStreamDriftController;
pub use mixer_stream_session::{MixerStreamSession, MixerStreamSessionError, StartedMixerStream};
pub use output_controller::{OutputController, OutputLifecycleError, OutputSelection};
pub use pairing_controller::{PairingInvitationController, PairingInvitationError};
pub use pairing_handshake::{
    MAX_HANDSHAKE_SOURCES, PairingHandshakeError, PairingHandshakeOutcome, PairingQrInvitation,
    ScannedProofAttempt, begin_from_rendered_payload, verify_scanned_proof,
};
pub use pairing_record::{
    MAX_PAIRING_ATTEMPTS_PER_SOURCE, PairingAttemptOutcome, PairingId, PairingProofVerification,
    PairingRecord, PairingRecordError, PairingRecordState, PairingSource,
};
pub use pcm_render_source::PcmRenderSource;
pub use receiver_output_controller::{
    ReceiverOutputController, ReceiverOutputLifecycleError, ReceiverPcmProducerInstaller,
};
pub use receiver_pcm_endpoint_factory::{
    ReceiverPcmEndpoint, ReceiverPcmEndpointError, ReceiverPcmEndpointFactory,
};
pub use receiver_pipeline::{
    AdaptiveTargetConfig, AdaptiveTargetSetupError, FreshSessionInstallError,
    ReceiverPipelineError, ReceiverStreamPipeline, TargetGatedPlayoutSetupError,
};
pub use reconnect_grace::{
    ReconnectGraceController, ReconnectGraceError, ReconnectGraceObservation, ReconnectGraceState,
};
pub use sample_rate_mapped_render_source::{
    DriftAdjustmentError, DriftAdjustmentHandle, SampleRateMappedRenderSource,
    SampleRateMappedRenderSourceError,
};
pub use session::{SessionEvent, SessionPhase, SessionState, SessionTransitionError};
pub use sonimesh_audio::{GainControlError, GainControlHandle, GainControlState, MAX_GAIN_LINEAR};
pub use stream_drift_controller::StreamDriftController;
pub use stream_format_adapter::{PumpOutcome, StreamFormatAdapter, StreamFormatAdapterError};
pub use udp_receiver_pipeline::{
    UdpPollOutcome, UdpReceiverError, UdpReceiverPipeline, UdpReceiverSetupError,
};
