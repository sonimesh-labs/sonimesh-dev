use core::fmt;
use std::io;

/// Debug-only I/O error view that excludes free-form operating-system text.
///
/// Socket and backend error messages may include raw endpoints or other
/// user-controlled values, so general debug logs retain only stable error
/// classification metadata.
pub(crate) struct RedactedIoError<'a>(pub &'a io::Error);

impl fmt::Debug for RedactedIoError<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RedactedIoError")
            .field("kind", &self.0.kind())
            .field("raw_os_error", &self.0.raw_os_error())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::RedactedIoError;

    #[test]
    fn debug_output_omits_the_free_form_error_message() {
        let error = io::Error::new(
            io::ErrorKind::ConnectionRefused,
            "connection to 192.168.10.42:49321 was refused",
        );

        let debug = format!("{:?}", RedactedIoError(&error));

        assert!(debug.contains("ConnectionRefused"));
        assert!(!debug.contains("192.168.10.42"));
        assert!(!debug.contains("was refused"));
    }
}
