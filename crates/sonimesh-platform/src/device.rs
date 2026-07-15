/// Opaque persistent platform output identifier.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OutputDeviceId(String);

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputDeviceInfo {
    /// Persistent platform identifier used for pinned-device selection.
    pub id: OutputDeviceId,
    /// User-facing operating-system description.
    pub display_name: String,
    /// Whether the endpoint was the default output during enumeration.
    pub is_default: bool,
}
