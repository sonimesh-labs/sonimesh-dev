use std::net::SocketAddr;

/// Replaces a network endpoint with a non-identifying family and port marker.
///
/// This helper is for control-domain logs and allocates a presentation string;
/// it must not be called from the realtime output callback.
#[must_use]
pub fn redact_endpoint(endpoint: SocketAddr) -> String {
    let family = if endpoint.is_ipv4() { "ipv4" } else { "ipv6" };
    format!("[{family}-address-redacted]:{}", endpoint.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_retain_ipv4_address() {
        let endpoint: SocketAddr = "192.168.10.42:49321".parse().unwrap();
        let redacted = redact_endpoint(endpoint);
        assert_eq!(redacted, "[ipv4-address-redacted]:49321");
        assert!(!redacted.contains("192.168"));
    }
}
