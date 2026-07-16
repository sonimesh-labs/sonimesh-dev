//! Desktop control-domain coordination, independent of Tauri commands.

mod control_stream;
mod output_controller;
mod pairing_controller;
mod pairing_record;
mod receiver_pipeline;
mod session;

pub use control_stream::{
    ControlStreamAction, ControlStreamCoordinator, ControlStreamError, MediaStreamIdentity,
    StreamStartRequest,
};
pub use output_controller::{OutputController, OutputLifecycleError, OutputSelection};
pub use pairing_controller::{PairingInvitationController, PairingInvitationError};
pub use pairing_record::{
    MAX_PAIRING_ATTEMPTS_PER_SOURCE, PairingAttemptOutcome, PairingId, PairingRecord,
    PairingRecordError, PairingRecordState, PairingSource,
};
pub use receiver_pipeline::{ReceiverPipelineError, ReceiverStreamPipeline};
pub use session::{SessionEvent, SessionPhase, SessionState, SessionTransitionError};
