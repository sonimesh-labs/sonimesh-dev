//! Desktop control-domain coordination, independent of Tauri commands.

mod output_controller;
mod session;

pub use output_controller::{OutputController, OutputLifecycleError, OutputSelection};
pub use session::{SessionEvent, SessionState, SessionTransitionError};
