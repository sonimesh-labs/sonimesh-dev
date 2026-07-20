//! Two-second silent direct-WASAPI output smoke test for Windows lab use.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;
    use std::time::Duration;

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

#[cfg(not(windows))]
fn main() {
    eprintln!("wasapi_silence_smoke requires Windows");
}
