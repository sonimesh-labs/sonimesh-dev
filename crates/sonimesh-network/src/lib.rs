//! Network-domain boundaries for SoniMesh.

mod ingress;

pub use ingress::{Authenticator, IngressError, MediaIngress, ReceivedFrame, StreamIdentity};
