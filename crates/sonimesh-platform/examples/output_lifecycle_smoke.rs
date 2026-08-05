//! Repeated silent CPAL output lifecycle smoke for explicit Windows lab use.

#[cfg(all(windows, feature = "cpal-output"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;

    use sonimesh_platform::{AudioOutput, CpalOutput, OutputHealth};

    let experiment = experiment_config()?;
    let mut output = CpalOutput::new();
    let probe = output.default_device_probe()?;
    let fixed_config = probe.output_config(Some(480), 8_192)?;

    println!("cycles: {}", experiment.cycles);
    println!(
        "active duration (ms): {}",
        experiment.active_duration.as_millis()
    );
    println!(
        "default format: {} Hz, {} channels, {}",
        probe.sample_rate_hz, probe.channels, probe.backend_sample_format
    );
    println!("maximum frames: {}", fixed_config.maximum_callback_frames);

    for cycle in 1..=experiment.cycles {
        let config = start_with_fixed_or_default(&mut output, fixed_config, cycle)?;
        thread::sleep(experiment.active_duration);
        let status = output.status();
        let health = output.health();
        output.stop();

        println!(
            "cycle {cycle}: request={}, callbacks={}, latest_frames={}, max_frames={}, latest_offset_ns={}, max_gap_ns={}, oversized={}, errors={}, health={health:?}",
            config
                .requested_callback_frames
                .map_or_else(|| "default".to_owned(), |frames| frames.to_string()),
            status.callback_count,
            status.last_callback_frames,
            status.maximum_callback_frames,
            status.last_callback_offset_ns,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExperimentConfig {
    cycles: u32,
    active_duration: std::time::Duration,
}

#[cfg(all(windows, feature = "cpal-output"))]
fn experiment_config() -> Result<ExperimentConfig, std::io::Error> {
    const DEFAULT_CYCLES: u32 = 5;
    const MAXIMUM_CYCLES: u32 = 500;
    const DEFAULT_ACTIVE_MILLIS: u64 = 250;
    const MAXIMUM_ACTIVE_MILLIS: u64 = 1_000;
    Ok(ExperimentConfig {
        cycles: environment_u32(
            "SONIMESH_OUTPUT_LIFECYCLE_CYCLES",
            DEFAULT_CYCLES,
            MAXIMUM_CYCLES,
        )?,
        active_duration: std::time::Duration::from_millis(environment_u64(
            "SONIMESH_OUTPUT_LIFECYCLE_ACTIVE_MILLIS",
            DEFAULT_ACTIVE_MILLIS,
            MAXIMUM_ACTIVE_MILLIS,
        )?),
    })
}

#[cfg(all(windows, feature = "cpal-output"))]
fn environment_u32(name: &str, default: u32, maximum: u32) -> Result<u32, std::io::Error> {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => {
            return invalid_input(name, "must be valid Unicode");
        }
    };
    parse_u32(&value, name, maximum)
}

#[cfg(all(windows, feature = "cpal-output"))]
fn environment_u64(name: &str, default: u64, maximum: u64) -> Result<u64, std::io::Error> {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => {
            return invalid_input(name, "must be valid Unicode");
        }
    };
    parse_u64(&value, name, maximum)
}

#[cfg(all(windows, feature = "cpal-output"))]
fn parse_u32(value: &str, name: &str, maximum: u32) -> Result<u32, std::io::Error> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| invalid_input_error(name, "must be numeric"))?;
    if !(1..=maximum).contains(&parsed) {
        return invalid_input(name, "is outside the accepted bound");
    }
    Ok(parsed)
}

#[cfg(all(windows, feature = "cpal-output"))]
fn parse_u64(value: &str, name: &str, maximum: u64) -> Result<u64, std::io::Error> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| invalid_input_error(name, "must be numeric"))?;
    if !(1..=maximum).contains(&parsed) {
        return invalid_input(name, "is outside the accepted bound");
    }
    Ok(parsed)
}

#[cfg(all(windows, feature = "cpal-output"))]
fn invalid_input<T>(name: &str, message: &str) -> Result<T, std::io::Error> {
    Err(invalid_input_error(name, message))
}

#[cfg(all(windows, feature = "cpal-output"))]
fn invalid_input_error(name: &str, message: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("{name} {message}"),
    )
}

#[cfg(all(windows, feature = "cpal-output"))]
fn start_with_fixed_or_default(
    output: &mut sonimesh_platform::CpalOutput,
    fixed_config: sonimesh_platform::OutputConfig,
    cycle: u32,
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
            println!("cycle {cycle}: fixed 480-frame request rejected: {error}");
            let default_config = OutputConfig {
                requested_callback_frames: None,
                ..fixed_config
            };
            output.start(None, default_config, Box::new(Silence))?;
            Ok(default_config)
        }
    }
}

#[cfg(all(test, windows, feature = "cpal-output"))]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_parameter_bounds_are_explicit() {
        assert_eq!(parse_u32("1", "cycles", 500).unwrap(), 1);
        assert_eq!(parse_u32("500", "cycles", 500).unwrap(), 500);
        assert!(parse_u32("0", "cycles", 500).is_err());
        assert!(parse_u32("501", "cycles", 500).is_err());
        assert_eq!(parse_u64("1", "duration", 1_000).unwrap(), 1);
        assert_eq!(parse_u64("1000", "duration", 1_000).unwrap(), 1_000);
        assert!(parse_u64("0", "duration", 1_000).is_err());
        assert!(parse_u64("1001", "duration", 1_000).is_err());
    }
}

#[cfg(not(all(windows, feature = "cpal-output")))]
fn main() {
    eprintln!("output_lifecycle_smoke requires Windows and --features cpal-output");
}
