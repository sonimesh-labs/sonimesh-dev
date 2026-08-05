//! Structured network-domain error source-chain tests.

use std::{error::Error, io};

use sonimesh_network::{
    IngressError, ProtectedIngressError, SourceEndpointAdmissionError, UdpReceiveError,
    UntrustedControlError,
};
use sonimesh_protocol::{ControlFrameError, MediaCryptoError, PacketParseError};

fn source<'a>(error: &'a (dyn Error + 'static)) -> &'a (dyn Error + 'static) {
    error
        .source()
        .expect("composite network error must retain its cause")
}

#[test]
fn ingress_errors_retain_parse_and_authentication_causes() {
    let parse = IngressError::<MediaCryptoError>::Parse(PacketParseError::Truncated);
    assert!(source(&parse).downcast_ref::<PacketParseError>().is_some());

    let authentication = IngressError::Authentication(MediaCryptoError::AuthenticationFailed);
    assert!(
        source(&authentication)
            .downcast_ref::<MediaCryptoError>()
            .is_some()
    );

    assert!(Error::source(&IngressError::<MediaCryptoError>::UnknownStream).is_none());
    assert!(Error::source(&IngressError::<MediaCryptoError>::InvalidPlaintextLength).is_none());
}

#[test]
fn protected_ingress_errors_retain_crypto_causes() {
    let error = ProtectedIngressError::Crypto(MediaCryptoError::KeyDerivationFailed);
    assert!(source(&error).downcast_ref::<MediaCryptoError>().is_some());
    assert!(Error::source(&ProtectedIngressError::RotationContextMismatch).is_none());
    assert!(Error::source(&ProtectedIngressError::NonIncreasingEpoch).is_none());
}

#[test]
fn untrusted_control_errors_retain_protocol_and_io_causes() {
    let protocol = UntrustedControlError::Protocol(ControlFrameError::TruncatedLength);
    assert!(
        source(&protocol)
            .downcast_ref::<ControlFrameError>()
            .is_some()
    );

    let io = UntrustedControlError::Io(io::Error::from(io::ErrorKind::PermissionDenied));
    let io_source = source(&io);
    assert_eq!(
        io_source
            .downcast_ref::<io::Error>()
            .expect("I/O wrapper must retain the operating-system cause")
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    assert!(Error::source(&UntrustedControlError::WritePending).is_none());
}

#[test]
fn udp_and_source_pin_errors_retain_wrapped_processing_causes() {
    let io = UdpReceiveError::Io(io::Error::from(io::ErrorKind::TimedOut));
    let io_source = source(&io);
    assert_eq!(
        io_source
            .downcast_ref::<io::Error>()
            .expect("UDP receive wrapper must retain the operating-system cause")
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert!(Error::source(&UdpReceiveError::DatagramTooLarge).is_none());

    let processing = SourceEndpointAdmissionError::Processing(UdpReceiveError::DatagramTooLarge);
    assert!(
        source(&processing)
            .downcast_ref::<UdpReceiveError>()
            .is_some()
    );
    assert!(
        Error::source(&SourceEndpointAdmissionError::<UdpReceiveError>::SourceEndpointChanged)
            .is_none()
    );
}
