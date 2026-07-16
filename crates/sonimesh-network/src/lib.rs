//! Network-domain boundaries for SoniMesh.

mod ingress;
mod protocol_authenticator;

pub use ingress::{Authenticator, IngressError, MediaIngress, ReceivedFrame, StreamIdentity};
pub use protocol_authenticator::ProtocolAuthenticator;
