//! Two-second silent CPAL output smoke test for explicit Windows lab use.

#[cfg(all(windows, feature = "cpal-output"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;
    use std::time::Duration;

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
    thread::sleep(Duration::from_secs(2));
    let status = output.status();
    output.stop();

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
    println!(
        "maximum interleaved samples: {}",
        status.maximum_callback_samples
    );
    println!("oversized callbacks: {}", status.oversized_callbacks);
    println!("stream errors: {}", status.stream_errors);

    if status.callback_count == 0 || status.stream_errors > 0 {
        return Err("silent output smoke test did not remain active".into());
    }
    Ok(())
}

#[cfg(not(all(windows, feature = "cpal-output")))]
fn main() {
    eprintln!("output_smoke requires Windows and --features cpal-output");
}
