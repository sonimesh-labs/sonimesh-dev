//! Network-domain boundaries for SoniMesh.

mod authenticated_control_stream;
mod desktop_tls;
mod ingress;
mod media_session_sender;
mod protected_ingress;
mod protocol_authenticator;
mod redacted_endpoint;
mod redacted_io_error;
mod source_endpoint_pin;
mod udp_receiver;
mod udp_sender;
mod untrusted_control_stream;

pub use authenticated_control_stream::{
    AuthenticatedControlError, AuthenticatedControlHandshake, AuthenticatedControlStream,
    VerifiedPeerProof, binding_for_channel,
};
pub use desktop_tls::{
    DesktopTlsError, PinnedClientChannel, PinnedServerChannel, accept_pinned_server, client_config,
    connect_pinned_client, server_config,
};
pub use ingress::{Authenticator, IngressError, MediaIngress, ReceivedFrame, StreamIdentity};
pub use media_session_sender::{
    MediaSessionSendError, MediaSessionSendOutcome, MediaSessionSender, MediaSessionSenderError,
};
pub use protected_ingress::{ProtectedIngressError, ProtectedMediaIngress};
pub use protocol_authenticator::ProtocolAuthenticator;
pub use rustls::ServerConfig;
pub use source_endpoint_pin::{SourceEndpointAdmissionError, SourceEndpointPin};
pub use udp_receiver::{ReceivedDatagram, UdpMediaReceiver, UdpReceiveError};
pub use udp_sender::{UdpMediaSender, UdpSendError};
pub use untrusted_control_stream::{
    UntrustedControlError, UntrustedControlPoll, UntrustedControlStream, UntrustedControlWrite,
};
