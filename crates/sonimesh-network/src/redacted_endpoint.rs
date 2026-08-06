use core::fmt;
use std::net::SocketAddr;

/// Debug-only endpoint view that excludes the host address.
///
/// Endpoint-family and port metadata are sufficient for low-rate diagnostics;
/// the raw address must not reach general debug logs.
pub(crate) struct RedactedEndpoint(pub SocketAddr);

impl fmt::Debug for RedactedEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let family = if self.0.is_ipv4() { "ipv4" } else { "ipv6" };
        formatter
            .debug_struct("RedactedEndpoint")
            .field("family", &family)
            .field("port", &self.0.port())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::RedactedEndpoint;

    #[test]
    fn debug_output_excludes_the_raw_ip_address() {
        let endpoint = "192.168.10.42:49321".parse().unwrap();
        let debug = format!("{endpoint:?}", endpoint = RedactedEndpoint(endpoint));

        assert_eq!(debug, r#"RedactedEndpoint { family: "ipv4", port: 49321 }"#);
        assert!(!debug.contains("192.168.10.42"));
    }
}
