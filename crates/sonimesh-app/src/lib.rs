//! Desktop control-domain coordination, independent of Tauri commands.

mod session;

pub use session::{SessionEvent, SessionState, SessionTransitionError};
