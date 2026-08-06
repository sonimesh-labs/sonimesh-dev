use core::fmt;
use std::net::SocketAddr;

use crate::redacted_endpoint::RedactedEndpoint;

/// Failure while applying the authenticated UDP source-endpoint policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceEndpointAdmissionError<E> {
    /// The candidate did not match the endpoint already pinned for this session.
    SourceEndpointChanged,
    /// The candidate endpoint matched, but protected processing rejected the packet.
    Processing(E),
}

impl<E: fmt::Display> fmt::Display for SourceEndpointAdmissionError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SourceEndpointChanged => formatter.write_str(
                "UDP media source endpoint changed before a fresh session was installed",
            ),
            Self::Processing(error) => {
                write!(formatter, "protected UDP processing failed: {error}")
            }
        }
    }
}

impl<E> std::error::Error for SourceEndpointAdmissionError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SourceEndpointChanged => None,
            Self::Processing(error) => Some(error),
        }
    }
}

/// Fail-closed source binding for one authenticated UDP media session.
///
/// A changed endpoint is rejected before `process` runs. An unbound endpoint is
/// committed only when `process` returns success, so parsing, authentication,
/// replay admission, and bounded jitter handling can remain transactional with
/// the source pin.
#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct SourceEndpointPin {
    pinned: Option<SocketAddr>,
}

impl fmt::Debug for SourceEndpointPin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("SourceEndpointPin");
        match self.pinned {
            Some(endpoint) => debug.field("pinned", &RedactedEndpoint(endpoint)),
            None => debug.field("pinned", &Option::<SocketAddr>::None),
        };
        debug.finish()
    }
}

impl SourceEndpointPin {
    /// Creates an unbound source policy for a fresh media session.
    #[must_use]
    pub const fn new() -> Self {
        Self { pinned: None }
    }

    /// Returns the endpoint pinned by the first successful protected outcome.
    #[must_use]
    pub const fn pinned(&self) -> Option<SocketAddr> {
        self.pinned
    }

    /// Applies source validation around one protected packet-processing attempt.
    ///
    /// `process` is not called for a changed endpoint. If this policy is unbound,
    /// the candidate endpoint is pinned only after `process` returns `Ok`.
    ///
    /// # Errors
    ///
    /// Returns [`SourceEndpointAdmissionError::SourceEndpointChanged`] before
    /// protected work for an unexpected endpoint, or wraps the processing error
    /// without changing the current binding.
    pub fn process<T, E>(
        &mut self,
        source: SocketAddr,
        process: impl FnOnce() -> Result<T, E>,
    ) -> Result<T, SourceEndpointAdmissionError<E>> {
        if self.pinned.is_some_and(|expected| expected != source) {
            return Err(SourceEndpointAdmissionError::SourceEndpointChanged);
        }

        let outcome = process().map_err(SourceEndpointAdmissionError::Processing)?;
        self.pinned.get_or_insert(source);
        Ok(outcome)
    }

    /// Clears the binding after a fresh authenticated media session is installed.
    pub fn clear_for_fresh_session(&mut self) {
        self.pinned = None;
    }
}

#[cfg(test)]
mod tests {
    use core::cell::Cell;

    use super::{SourceEndpointAdmissionError, SourceEndpointPin};

    fn endpoint(port: u16) -> std::net::SocketAddr {
        ([192, 0, 2, 1], port).into()
    }

    #[test]
    fn failed_processing_does_not_pin_an_unbound_source() {
        let mut pin = SourceEndpointPin::new();

        assert_eq!(
            pin.process(endpoint(4000), || Err::<(), _>("rejected")),
            Err(SourceEndpointAdmissionError::Processing("rejected"))
        );
        assert_eq!(pin.pinned(), None);
    }

    #[test]
    fn changed_source_is_rejected_before_processing() {
        let mut pin = SourceEndpointPin::new();
        assert_eq!(pin.process(endpoint(4000), || Ok::<_, ()>(7)), Ok(7));
        let called = Cell::new(false);

        assert_eq!(
            pin.process(endpoint(4001), || {
                called.set(true);
                Ok::<_, ()>(8)
            }),
            Err(SourceEndpointAdmissionError::SourceEndpointChanged)
        );
        assert!(!called.get());
        assert_eq!(pin.pinned(), Some(endpoint(4000)));
    }

    #[test]
    fn pinned_source_can_continue_processing() {
        let mut pin = SourceEndpointPin::new();
        assert_eq!(pin.process(endpoint(4000), || Ok::<_, ()>(7)), Ok(7));

        assert_eq!(pin.process(endpoint(4000), || Ok::<_, ()>(8)), Ok(8));
        assert_eq!(pin.pinned(), Some(endpoint(4000)));
    }

    #[test]
    fn failed_processing_from_the_pinned_source_preserves_the_binding() {
        let mut pin = SourceEndpointPin::new();
        assert_eq!(pin.process(endpoint(4000), || Ok::<_, ()>(())), Ok(()));

        assert_eq!(
            pin.process(endpoint(4000), || Err::<(), _>("rejected")),
            Err(SourceEndpointAdmissionError::Processing("rejected"))
        );
        assert_eq!(pin.pinned(), Some(endpoint(4000)));

        let called = Cell::new(false);
        assert_eq!(
            pin.process(endpoint(4001), || {
                called.set(true);
                Ok::<_, ()>(())
            }),
            Err(SourceEndpointAdmissionError::SourceEndpointChanged)
        );
        assert!(!called.get());
    }

    #[test]
    fn debug_output_redacts_the_pinned_ip_address() {
        let mut pin = SourceEndpointPin::new();
        let endpoint = "192.168.10.42:49321".parse().unwrap();
        assert_eq!(pin.process(endpoint, || Ok::<_, ()>(())), Ok(()));

        let debug = format!("{pin:?}");

        assert!(debug.contains("ipv4"));
        assert!(debug.contains("49321"));
        assert!(!debug.contains("192.168.10.42"));
    }

    #[test]
    fn fresh_session_clear_allows_a_new_source_to_bind() {
        let mut pin = SourceEndpointPin::new();
        assert_eq!(pin.process(endpoint(4000), || Ok::<_, ()>(())), Ok(()));

        pin.clear_for_fresh_session();

        assert_eq!(pin.pinned(), None);
        assert_eq!(pin.process(endpoint(4001), || Ok::<_, ()>(())), Ok(()));
        assert_eq!(pin.pinned(), Some(endpoint(4001)));
    }
}
