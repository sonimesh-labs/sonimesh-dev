//! Repeated silent direct-WASAPI output lifecycle smoke for Windows lab use.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;

    use sonimesh_platform::{
        AudioOutput, OutputConfig, OutputDeviceId, OutputHealth, RenderSource,
    };
    use windows_wasapi_prototype::WasapiOutput;

    struct Silence;

    impl RenderSource for Silence {
        fn render(&mut self, output: &mut [f32]) {
            output.fill(0.0);
        }
    }

    let experiment = experiment_config()?;
    let mut output = WasapiOutput::new();
    let pinned_id = lifecycle_device_id()?;
    let device = pinned_id.as_ref().map(|id| OutputDeviceId::new(id.clone()));
    let fixed_config = output.output_config_for_device(device.as_ref(), Some(480), 8_192)?;
    println!("cycles: {}", experiment.cycles);
    println!(
        "active duration (ms): {}",
        experiment.active_duration.as_millis()
    );
    println!(
        "device selection: {}",
        if device.is_some() {
            "pinned"
        } else {
            "default"
        }
    );
    println!(
        "default format: {} Hz, {} channels, {:?}",
        fixed_config.sample_rate_hz, fixed_config.channels, fixed_config.sample_format
    );
    println!("maximum frames: {}", fixed_config.maximum_callback_frames);

    for cycle in 1..=experiment.cycles {
        let config = match output.start(device.as_ref(), fixed_config, Box::new(Silence)) {
            Ok(()) => fixed_config,
            Err(error) => {
                println!("cycle {cycle}: fixed 480-frame request rejected: {error}");
                let default_config = OutputConfig {
                    requested_callback_frames: None,
                    ..fixed_config
                };
                output.start(device.as_ref(), default_config, Box::new(Silence))?;
                default_config
            }
        };
        thread::sleep(experiment.active_duration);
        let status = output.status();
        let health = output.health();
        output.stop();

        println!(
            "cycle {cycle}: request={}, renders={}, latest_frames={}, max_frames={}, latest_offset_ns={}, max_gap_ns={}, primed_frames={}, oversized={}, errors={}, health={health:?}",
            config
                .requested_callback_frames
                .map_or_else(|| "default".to_owned(), |frames| frames.to_string()),
            status.render_count,
            status.last_render_frames,
            status.maximum_render_frames,
            status.last_render_offset_ns,
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

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ExperimentConfig {
    cycles: u32,
    active_duration: std::time::Duration,
}

#[cfg(windows)]
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

#[cfg(windows)]
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

#[cfg(windows)]
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

#[cfg(windows)]
fn parse_u32(value: &str, name: &str, maximum: u32) -> Result<u32, std::io::Error> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| invalid_input_error(name, "must be numeric"))?;
    if !(1..=maximum).contains(&parsed) {
        return invalid_input(name, "is outside the accepted bound");
    }
    Ok(parsed)
}

#[cfg(windows)]
fn parse_u64(value: &str, name: &str, maximum: u64) -> Result<u64, std::io::Error> {
    let parsed = value
        .parse::<u64>()
        .map_err(|_| invalid_input_error(name, "must be numeric"))?;
    if !(1..=maximum).contains(&parsed) {
        return invalid_input(name, "is outside the accepted bound");
    }
    Ok(parsed)
}

#[cfg(windows)]
fn invalid_input<T>(name: &str, message: &str) -> Result<T, std::io::Error> {
    Err(invalid_input_error(name, message))
}

#[cfg(windows)]
fn invalid_input_error(name: &str, message: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        format!("{name} {message}"),
    )
}

#[cfg(windows)]
fn lifecycle_device_id() -> Result<Option<String>, std::io::Error> {
    match std::env::var("SONIMESH_OUTPUT_LIFECYCLE_DEVICE_ID") {
        Ok(value) => validate_lifecycle_device_id(value).map(Some),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SONIMESH_OUTPUT_LIFECYCLE_DEVICE_ID must be valid Unicode",
        )),
    }
}

#[cfg(windows)]
fn validate_lifecycle_device_id(value: String) -> Result<String, std::io::Error> {
    if value.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "SONIMESH_OUTPUT_LIFECYCLE_DEVICE_ID must not be empty",
        ));
    }
    Ok(value)
}

#[cfg(all(test, windows))]
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

    #[test]
    fn lifecycle_device_identifier_bounds_are_explicit() {
        assert_eq!(
            validate_lifecycle_device_id("wasapi:endpoint".to_owned()).unwrap(),
            "wasapi:endpoint"
        );
        assert!(validate_lifecycle_device_id(String::new()).is_err());
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("wasapi_lifecycle_smoke requires Windows");
}
