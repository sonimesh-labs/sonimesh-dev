//! Repeated silent direct-WASAPI output lifecycle smoke for Windows lab use.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;
    use std::time::Duration;

    use sonimesh_platform::{AudioOutput, OutputConfig, OutputHealth, RenderSource};
    use windows_wasapi_prototype::WasapiOutput;

    const CYCLES: u32 = 5;
    const ACTIVE_DURATION: Duration = Duration::from_millis(250);

    struct Silence;

    impl RenderSource for Silence {
        fn render(&mut self, output: &mut [f32]) {
            output.fill(0.0);
        }
    }

    let mut output = WasapiOutput::new();
    let fixed_config = output.default_output_config(Some(480), 8_192)?;
    println!("cycles: {CYCLES}");
    println!(
        "default format: {} Hz, {} channels, {:?}",
        fixed_config.sample_rate_hz, fixed_config.channels, fixed_config.sample_format
    );

    for cycle in 1..=CYCLES {
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
        thread::sleep(ACTIVE_DURATION);
        let status = output.status();
        let health = output.health();
        output.stop();

        println!(
            "cycle {cycle}: request={}, renders={}, latest_frames={}, max_frames={}, max_gap_ns={}, primed_frames={}, oversized={}, errors={}, health={health:?}",
            config
                .requested_callback_frames
                .map_or_else(|| "default".to_owned(), |frames| frames.to_string()),
            status.render_count,
            status.last_render_frames,
            status.maximum_render_frames,
            status.maximum_render_gap_ns,
            status.primed_silence_frames,
            status.oversized_renders,
            status.stream_errors,
        );

        if status.render_count == 0
            || status.oversized_renders > 0
            || status.stream_errors > 0
            || health != OutputHealth::Healthy
        {
            return Err(format!("cycle {cycle} did not remain healthy").into());
        }
    }

    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("wasapi_lifecycle_smoke requires Windows");
}
