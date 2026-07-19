//! Repeated silent CPAL output lifecycle smoke for explicit Windows lab use.

#[cfg(all(windows, feature = "cpal-output"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;
    use std::time::Duration;

    use sonimesh_platform::{AudioOutput, CpalOutput, OutputHealth};

    const CYCLES: u32 = 5;
    const ACTIVE_DURATION: Duration = Duration::from_millis(250);

    let mut output = CpalOutput::new();
    let probe = output.default_device_probe()?;
    let fixed_config = probe.output_config(Some(480), 8_192)?;

    println!("cycles: {CYCLES}");
    println!(
        "default format: {} Hz, {} channels, {}",
        probe.sample_rate_hz, probe.channels, probe.backend_sample_format
    );

    for cycle in 1..=CYCLES {
        let config = start_with_fixed_or_default(&mut output, fixed_config)?;
        thread::sleep(ACTIVE_DURATION);
        let status = output.status();
        let health = output.health();
        output.stop();

        println!(
            "cycle {cycle}: request={}, callbacks={}, latest_frames={}, max_frames={}, max_gap_ns={}, oversized={}, errors={}, health={health:?}",
            config
                .requested_callback_frames
                .map_or_else(|| "default".to_owned(), |frames| frames.to_string()),
            status.callback_count,
            status.last_callback_frames,
            status.maximum_callback_frames,
            status.maximum_callback_gap_ns,
            status.oversized_callbacks,
            status.stream_errors,
        );

        if status.callback_count == 0
            || status.oversized_callbacks > 0
            || status.stream_errors > 0
            || health != OutputHealth::Healthy
        {
            return Err(format!("cycle {cycle} did not remain healthy").into());
        }
    }

    Ok(())
}

#[cfg(all(windows, feature = "cpal-output"))]
fn start_with_fixed_or_default(
    output: &mut sonimesh_platform::CpalOutput,
    fixed_config: sonimesh_platform::OutputConfig,
) -> Result<sonimesh_platform::OutputConfig, sonimesh_platform::OutputError> {
    use sonimesh_platform::{AudioOutput, OutputConfig, RenderSource};

    struct Silence;

    impl RenderSource for Silence {
        fn render(&mut self, output: &mut [f32]) {
            output.fill(0.0);
        }
    }

    match output.start(None, fixed_config, Box::new(Silence)) {
        Ok(()) => Ok(fixed_config),
        Err(error) => {
            println!("fixed 480-frame request rejected: {error}");
            let default_config = OutputConfig {
                requested_callback_frames: None,
                ..fixed_config
            };
            output.start(None, default_config, Box::new(Silence))?;
            Ok(default_config)
        }
    }
}

#[cfg(not(all(windows, feature = "cpal-output")))]
fn main() {
    eprintln!("output_lifecycle_smoke requires Windows and --features cpal-output");
}
