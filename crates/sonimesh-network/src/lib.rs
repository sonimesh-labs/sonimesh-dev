//! Network-domain boundaries for SoniMesh.

mod ingress;
mod protected_ingress;
mod protocol_authenticator;
mod source_endpoint_pin;
mod udp_receiver;
mod untrusted_control_stream;

pub use ingress::{Authenticator, IngressError, MediaIngress, ReceivedFrame, StreamIdentity};
pub use protected_ingress::{ProtectedIngressError, ProtectedMediaIngress};
pub use protocol_authenticator::ProtocolAuthenticator;
pub use source_endpoint_pin::{SourceEndpointAdmissionError, SourceEndpointPin};
pub use udp_receiver::{ReceivedDatagram, UdpMediaReceiver, UdpReceiveError};
pub use untrusted_control_stream::{
    UntrustedControlError, UntrustedControlPoll, UntrustedControlStream, UntrustedControlWrite,
};
