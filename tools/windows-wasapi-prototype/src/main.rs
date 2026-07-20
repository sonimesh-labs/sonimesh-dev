//! Two-second silent direct-WASAPI output smoke test for Windows lab use.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;

    use sonimesh_platform::{AudioOutput, OutputConfig, RenderSource};
    use windows_wasapi_prototype::WasapiOutput;

    struct Silence;

    impl RenderSource for Silence {
        fn render(&mut self, output: &mut [f32]) {
            output.fill(0.0);
        }
    }

    let mut output = WasapiOutput::new();
    let fixed_config = output.default_output_config(Some(480), 8_192)?;
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
}

#[cfg(not(windows))]
fn main() {
    eprintln!("wasapi_silence_smoke requires Windows");
}
