//! Desktop control-domain coordination, independent of Tauri commands.

mod control_stream;
mod output_controller;
mod session;

pub use control_stream::{
    ControlStreamAction, ControlStreamCoordinator, ControlStreamError, MediaStreamIdentity,
    StreamStartRequest,
};
pub use output_controller::{OutputController, OutputLifecycleError, OutputSelection};
pub use session::{SessionEvent, SessionState, SessionTransitionError};
