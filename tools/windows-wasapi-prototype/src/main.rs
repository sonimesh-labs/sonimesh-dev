//! Two-second silent direct-WASAPI output smoke test for Windows lab use.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;

    use sonimesh_platform::{AudioOutput, OutputConfig, OutputDeviceId, RenderSource};
    use windows_wasapi_prototype::WasapiOutput;

    struct Silence;

    impl RenderSource for Silence {
        fn render(&mut self, output: &mut [f32]) {
            output.fill(0.0);
        }
    }

    let mut output = WasapiOutput::new();
    let pinned_id = smoke_device_id()?;
    let device = pinned_id.as_ref().map(|id| OutputDeviceId::new(id.clone()));
    let fixed_config = output.output_config_for_device(device.as_ref(), Some(480), 8_192)?;
    let config = match output.start(device.as_ref(), fixed_config, Box::new(Silence)) {
        Ok(()) => fixed_config,
        Err(error) => {
            println!("fixed 480-frame request rejected: {error}");
            let default_config = OutputConfig {
                requested_callback_frames: None,
                ..fixed_config
            };
            output.start(device.as_ref(), default_config, Box::new(Silence))?;
            default_config
        }
    };
    println!(
        "device selection: {}",
        if device.is_some() {
            "pinned"
        } else {
            "default"
        }
    );
    println!(
        "configured format: {} Hz, {} channels, {:?}",
        config.sample_rate_hz, config.channels, config.sample_format
    );

    let active_duration = smoke_duration()?;
    thread::sleep(active_duration);
    let status = output.status();
    output.stop();

    println!("active duration (s): {}", active_duration.as_secs());
    println!(
        "requested frames: {}",
        config
            .requested_callback_frames
            .map_or_else(|| "default".to_owned(), |frames| frames.to_string())
    );
    println!("maximum frames: {}", config.maximum_callback_frames);
    println!("requested channels: {}", config.channels);
    println!("endpoint buffer frames: {}", status.endpoint_buffer_frames);
    println!("primed silence frames: {}", status.primed_silence_frames);
    println!("render cycles: {}", status.render_count);
    println!("latest render frames: {}", status.last_render_frames);
    println!("maximum render frames: {}", status.maximum_render_frames);
    println!(
        "latest render offset (ns): {}",
        status.last_render_offset_ns
    );
    println!("largest render gap (ns): {}", status.maximum_render_gap_ns);
    println!("oversized renders: {}", status.oversized_renders);
    println!("stream errors: {}", status.stream_errors);

    if status.render_count == 0 || status.stream_errors > 0 {
        return Err("direct WASAPI silent output smoke test did not remain active".into());
    }
    Ok(())
}

/// Reads the optional pinned endpoint for this smoke run.
///
/// An absent variable selects the current Windows default render endpoint.
#[cfg(windows)]
fn smoke_device_id() -> Result<Option<String>, std::io::Error> {
    match std::env::var("SONIMESH_OUTPUT_SMOKE_DEVICE_ID") {
        Ok(value) => validate_smoke_device_id(value).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SONIMESH_OUTPUT_SMOKE_DEVICE_ID must be valid Unicode",
        )),
    }
}

#[cfg(windows)]
fn validate_smoke_device_id(value: String) -> Result<String, std::io::Error> {
    if value.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SONIMESH_OUTPUT_SMOKE_DEVICE_ID must not be empty",
        ));
    }
    Ok(value)
}

#[cfg(windows)]
fn smoke_duration() -> Result<std::time::Duration, std::io::Error> {
    const DEFAULT_SECONDS: u64 = 2;
    const MAXIMUM_SECONDS: u64 = 28_800;

    let seconds = match std::env::var("SONIMESH_OUTPUT_SMOKE_SECONDS") {
        Ok(value) => smoke_duration_from_value(&value)?.as_secs(),
        Err(std::env::VarError::NotPresent) => DEFAULT_SECONDS,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "SONIMESH_OUTPUT_SMOKE_SECONDS must be valid Unicode",
            ));
        }
    };
    debug_assert!(seconds <= MAXIMUM_SECONDS);
    Ok(std::time::Duration::from_secs(seconds))
}

#[cfg(windows)]
fn smoke_duration_from_value(value: &str) -> Result<std::time::Duration, std::io::Error> {
    const MAXIMUM_SECONDS: u64 = 28_800;

    let seconds = value.parse::<u64>().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SONIMESH_OUTPUT_SMOKE_SECONDS must be an integer from 1 through 28800",
        )
    })?;
    if !(1..=MAXIMUM_SECONDS).contains(&seconds) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SONIMESH_OUTPUT_SMOKE_SECONDS must be an integer from 1 through 28800",
        ));
    }
    Ok(std::time::Duration::from_secs(seconds))
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn smoke_duration_bounds_are_explicit() {
        assert!(smoke_duration_from_value("1").is_ok());
        assert!(smoke_duration_from_value("28800").is_ok());
        assert!(smoke_duration_from_value("0").is_err());
        assert!(smoke_duration_from_value("28801").is_err());
        assert!(smoke_duration_from_value("ten").is_err());
    }

    #[test]
    fn smoke_device_identifier_bounds_are_explicit() {
        assert_eq!(
            validate_smoke_device_id("wasapi:endpoint".to_owned()).unwrap(),
            "wasapi:endpoint"
        );
        assert!(validate_smoke_device_id(String::new()).is_err());
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("wasapi_silence_smoke requires Windows");
}
