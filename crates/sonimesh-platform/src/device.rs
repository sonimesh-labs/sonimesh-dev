use core::fmt;

/// Opaque persistent platform output identifier.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct OutputDeviceId(String);

impl fmt::Debug for OutputDeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("OutputDeviceId([REDACTED])")
    }
}

impl OutputDeviceId {
    /// Creates an identifier returned by a platform enumerator.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrows the platform-specific value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Untrusted presentation metadata for one output endpoint.
#[derive(Clone, Eq, PartialEq)]
pub struct OutputDeviceInfo {
    /// Persistent platform identifier used for pinned-device selection.
    pub id: OutputDeviceId,
    /// User-facing operating-system description.
    pub display_name: String,
    /// Whether the endpoint was the default output during enumeration.
    pub is_default: bool,
}

impl fmt::Debug for OutputDeviceInfo {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OutputDeviceInfo")
            .field("id", &self.id)
            .field("display_name_present", &(!self.display_name.is_empty()))
            .field("is_default", &self.is_default)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{OutputDeviceId, OutputDeviceInfo};

    #[test]
    fn debug_output_redacts_persistent_id_and_display_name() {
        let info = OutputDeviceInfo {
            id: OutputDeviceId::new("wasapi:{persistent-device-id}"),
            display_name: "Private Headphones".to_owned(),
            is_default: true,
        };

        let debug = format!("{info:?}");

        assert!(debug.contains("OutputDeviceId([REDACTED])"));
        assert!(debug.contains("display_name_present: true"));
        assert!(!debug.contains("persistent-device-id"));
        assert!(!debug.contains("Private Headphones"));
    }
}
