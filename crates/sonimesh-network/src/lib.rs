//! Network-domain boundaries for SoniMesh.

mod ingress;
mod protected_ingress;
mod protocol_authenticator;

pub use ingress::{Authenticator, IngressError, MediaIngress, ReceivedFrame, StreamIdentity};
pub use protected_ingress::{ProtectedIngressError, ProtectedMediaIngress};
pub use protocol_authenticator::ProtocolAuthenticator;
