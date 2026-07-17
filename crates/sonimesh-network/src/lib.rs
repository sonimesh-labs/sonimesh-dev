//! Network-domain boundaries for SoniMesh.

mod ingress;
mod protected_ingress;
mod protocol_authenticator;
mod udp_receiver;

pub use ingress::{Authenticator, IngressError, MediaIngress, ReceivedFrame, StreamIdentity};
pub use protected_ingress::{ProtectedIngressError, ProtectedMediaIngress};
pub use protocol_authenticator::ProtocolAuthenticator;
pub use udp_receiver::{ReceivedDatagram, UdpMediaReceiver, UdpReceiveError};
