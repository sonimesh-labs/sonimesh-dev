//! Long-running silent CPAL output soak for explicit Windows lab use.

#[cfg(all(windows, feature = "cpal-output"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::cmp;
    use std::thread;
    use std::time::Instant;

    use sonimesh_platform::{AudioOutput, CpalOutput, OutputConfig, OutputHealth, RenderSource};

    struct Silence;

    impl RenderSource for Silence {
        fn render(&mut self, output: &mut [f32]) {
            output.fill(0.0);
        }
    }

    let soak = soak_config()?;
    let mut output = CpalOutput::new();
    let probe = output.default_device_probe()?;
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

    println!("soak duration (s): {}", soak.duration.as_secs());
    println!("report interval (s): {}", soak.report_interval.as_secs());
    println!(
        "default format: {} Hz, {} channels, {}",
        probe.sample_rate_hz, probe.channels, probe.backend_sample_format
    );
    println!(
        "requested frames: {}",
        config
            .requested_callback_frames
            .map_or_else(|| "default".to_owned(), |frames| frames.to_string())
    );
    println!("maximum frames: {}", config.maximum_callback_frames);

    let started_at = Instant::now();
    let mut healthy = true;
    loop {
        let elapsed = started_at.elapsed();
        if elapsed >= soak.duration {
            break;
        }
        thread::sleep(cmp::min(soak.report_interval, soak.duration - elapsed));

        let status = output.status();
        let health = output.health();
        print_status(started_at.elapsed(), status, health);
        if status.oversized_callbacks > 0
            || status.stream_errors > 0
            || health != OutputHealth::Healthy
        {
            healthy = false;
            break;
        }
    }

    let status = output.status();
    let health = output.health();
    print!("final ");
    print_status(started_at.elapsed(), status, health);
    output.stop();

    if status.callback_count == 0
        || !healthy
        || status.oversized_callbacks > 0
        || status.stream_errors > 0
    {
        return Err("CPAL silent soak did not remain healthy".into());
    }
    Ok(())
}

#[cfg(all(windows, feature = "cpal-output"))]
fn print_status(
    elapsed: std::time::Duration,
    status: sonimesh_platform::CpalOutputStatus,
    health: sonimesh_platform::OutputHealth,
) {
    println!(
        "elapsed_s={}, callbacks={}, latest_frames={}, max_frames={}, latest_offset_ns={}, max_gap_ns={}, oversized={}, errors={}, health={health:?}",
        elapsed.as_secs(),
        status.callback_count,
        status.last_callback_frames,
        status.maximum_callback_frames,
        status.last_callback_offset_ns,
        status.maximum_callback_gap_ns,
        status.oversized_callbacks,
        status.stream_errors,
    );
}

#[cfg(all(windows, feature = "cpal-output"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SoakConfig {
    duration: std::time::Duration,
    report_interval: std::time::Duration,
}

#[cfg(all(windows, feature = "cpal-output"))]
fn soak_config() -> Result<SoakConfig, std::io::Error> {
    const DEFAULT_DURATION_SECONDS: u64 = 28_800;
    const DEFAULT_REPORT_SECONDS: u64 = 60;
    let duration_seconds = environment_seconds(
        "SONIMESH_OUTPUT_SOAK_SECONDS",
        DEFAULT_DURATION_SECONDS,
        DEFAULT_DURATION_SECONDS,
    )?;
    let report_seconds = environment_seconds(
        "SONIMESH_OUTPUT_SOAK_REPORT_SECONDS",
        DEFAULT_REPORT_SECONDS.min(duration_seconds),
        duration_seconds,
    )?;
    Ok(SoakConfig {
        duration: std::time::Duration::from_secs(duration_seconds),
        report_interval: std::time::Duration::from_secs(report_seconds),
    })
}

#[cfg(all(windows, feature = "cpal-output"))]
fn environment_seconds(
    name: &str,
    default_seconds: u64,
    maximum_seconds: u64,
) -> Result<u64, std::io::Error> {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(default_seconds),
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{name} must be valid Unicode"),
            ));
        }
    };
    parse_seconds(&value, name, maximum_seconds)
}

#[cfg(all(windows, feature = "cpal-output"))]
fn parse_seconds(value: &str, name: &str, maximum_seconds: u64) -> Result<u64, std::io::Error> {
    let seconds = value.parse::<u64>().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{name} must be an integer from 1 through {maximum_seconds}"),
        )
    })?;
    if !(1..=maximum_seconds).contains(&seconds) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{name} must be an integer from 1 through {maximum_seconds}"),
        ));
    }
    Ok(seconds)
}

#[cfg(all(test, windows, feature = "cpal-output"))]
mod tests {
    use super::*;

    #[test]
    fn environment_seconds_rejects_outside_the_requested_bound() {
        assert_eq!(parse_seconds("1", "duration", 60).unwrap(), 1);
        assert_eq!(parse_seconds("60", "duration", 60).unwrap(), 60);
        assert!(parse_seconds("0", "duration", 60).is_err());
        assert!(parse_seconds("61", "duration", 60).is_err());
    }
}

#[cfg(not(all(windows, feature = "cpal-output")))]
fn main() {
    eprintln!("output_soak requires Windows and --features cpal-output");
}
