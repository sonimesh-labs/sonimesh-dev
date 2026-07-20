//! Two-second silent CPAL output smoke test for explicit Windows lab use.

#[cfg(all(windows, feature = "cpal-output"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;

    use sonimesh_platform::{AudioOutput, CpalOutput, OutputConfig, RenderSource};

    struct Silence;

    impl RenderSource for Silence {
        fn render(&mut self, output: &mut [f32]) {
            output.fill(0.0);
        }
    }

    let mut output = CpalOutput::new();
    let probe = output.default_device_probe()?;
    println!("default device: {}", probe.device.display_name);
    println!(
        "default format: {} Hz, {} channels, {}",
        probe.sample_rate_hz, probe.channels, probe.backend_sample_format
    );
    let fixed_config = probe.output_config(Some(480), 8_192)?;
    let config = match output.start(None, fixed_config, Box::new(Silence)) {
        Ok(()) => fixed_config,
        Err(error) => {
            println!("fixed 480-frame request rejected: {error}");
            let default_config = OutputConfig {
                requested_callback_frames: None,
                ..fixed_config
            };
            output.start(None, default_config, Box::new(Silence))?;
            default_config
        }
    };
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
    println!("callbacks: {}", status.callback_count);
    println!(
        "latest interleaved samples: {}",
        status.last_callback_samples
    );
    println!("latest callback frames: {}", status.last_callback_frames);
    println!(
        "maximum interleaved samples: {}",
        status.maximum_callback_samples
    );
    println!(
        "maximum callback frames: {}",
        status.maximum_callback_frames
    );
    println!(
        "latest callback offset (ns): {}",
        status.last_callback_offset_ns
    );
    println!(
        "largest callback gap (ns): {}",
        status.maximum_callback_gap_ns
    );
    println!("oversized callbacks: {}", status.oversized_callbacks);
    println!("stream errors: {}", status.stream_errors);

    if status.callback_count == 0 || status.stream_errors > 0 {
        return Err("silent output smoke test did not remain active".into());
    }
    Ok(())
}

#[cfg(all(windows, feature = "cpal-output"))]
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

#[cfg(all(windows, feature = "cpal-output"))]
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

#[cfg(all(test, windows, feature = "cpal-output"))]
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
}

#[cfg(not(all(windows, feature = "cpal-output")))]
fn main() {
    eprintln!("output_smoke requires Windows and --features cpal-output");
}
