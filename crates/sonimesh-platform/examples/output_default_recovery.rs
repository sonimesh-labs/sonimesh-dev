//! Manual default-role change and CPAL output recovery experiment for Windows.

#[cfg(all(windows, feature = "cpal-output"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;

    use sonimesh_platform::{AudioOutput, CpalOutput, OutputHealth};

    let experiment = experiment_config()?;
    let mut output = CpalOutput::new();
    let initial_probe = output.default_device_probe()?;
    let initial_config = initial_probe.output_config(Some(480), 8_192)?;
    start_with_fixed_or_default(&mut output, initial_config)?;

    println!("initial format: {}", format_label(&initial_probe));
    println!(
        "wait for default change (s): {}",
        experiment.change_wait.as_secs()
    );
    println!("require default change: {}", experiment.require_change);
    thread::sleep(experiment.change_wait);

    let before_stop = output.status();
    let before_stop_health = output.health();
    print_status("before_stop", before_stop, before_stop_health);
    output.stop();

    let recovered_probe = output.default_device_probe()?;
    let changed = initial_probe.device.id != recovered_probe.device.id;
    println!("recovered format: {}", format_label(&recovered_probe));
    println!("default changed: {changed}");
    if experiment.require_change && !changed {
        return Err("Windows default render endpoint did not change during the experiment".into());
    }

    let recovered_config = recovered_probe.output_config(Some(480), 8_192)?;
    let recovered_config = start_with_fixed_or_default(&mut output, recovered_config)?;
    println!(
        "recovered request: {}",
        recovered_config
            .requested_callback_frames
            .map_or_else(|| "default".to_owned(), |frames| frames.to_string())
    );
    println!(
        "recovery active duration (s): {}",
        experiment.recovery_duration.as_secs()
    );
    thread::sleep(experiment.recovery_duration);

    let recovered_status = output.status();
    let recovered_health = output.health();
    print_status("recovered", recovered_status, recovered_health);
    output.stop();

    if before_stop.callback_count == 0
        || recovered_status.callback_count == 0
        || before_stop.oversized_callbacks > 0
        || recovered_status.oversized_callbacks > 0
        || before_stop.stream_errors > 0
        || recovered_status.stream_errors > 0
        || before_stop_health != OutputHealth::Healthy
        || recovered_health != OutputHealth::Healthy
    {
        return Err("CPAL default-role recovery experiment did not remain healthy".into());
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

#[cfg(all(windows, feature = "cpal-output"))]
fn format_label(probe: &sonimesh_platform::CpalDeviceProbe) -> String {
    format!(
        "{} Hz, {} channels, {}",
        probe.sample_rate_hz, probe.channels, probe.backend_sample_format
    )
}

#[cfg(all(windows, feature = "cpal-output"))]
fn print_status(
    stage: &str,
    status: sonimesh_platform::CpalOutputStatus,
    health: sonimesh_platform::OutputHealth,
) {
    println!(
        "stage={stage}, callbacks={}, latest_frames={}, max_frames={}, latest_offset_ns={}, max_gap_ns={}, oversized={}, errors={}, health={health:?}",
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
struct ExperimentConfig {
    change_wait: std::time::Duration,
    recovery_duration: std::time::Duration,
    require_change: bool,
}

#[cfg(all(windows, feature = "cpal-output"))]
fn experiment_config() -> Result<ExperimentConfig, std::io::Error> {
    Ok(ExperimentConfig {
        change_wait: std::time::Duration::from_secs(environment_seconds(
            "SONIMESH_OUTPUT_DEFAULT_CHANGE_WAIT_SECONDS",
            30,
            600,
        )?),
        recovery_duration: std::time::Duration::from_secs(environment_seconds(
            "SONIMESH_OUTPUT_RECOVERY_SECONDS",
            2,
            60,
        )?),
        require_change: environment_boolean("SONIMESH_OUTPUT_REQUIRE_DEFAULT_CHANGE", true)?,
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
            return invalid_input(name, "must be valid Unicode");
        }
    };
    parse_seconds(&value, name, maximum_seconds)
}

#[cfg(all(windows, feature = "cpal-output"))]
fn environment_boolean(name: &str, default: bool) -> Result<bool, std::io::Error> {
    let value = match std::env::var(name) {
        Ok(value) => value,
        Err(std::env::VarError::NotPresent) => return Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => {
            return invalid_input(name, "must be valid Unicode");
        }
    };
    parse_boolean(&value, name)
}

#[cfg(all(windows, feature = "cpal-output"))]
fn parse_seconds(value: &str, name: &str, maximum: u64) -> Result<u64, std::io::Error> {
    let seconds = value
        .parse::<u64>()
        .map_err(|_| invalid_input_error(name, "must be numeric"))?;
    if !(1..=maximum).contains(&seconds) {
        return invalid_input(name, "is outside the accepted bound");
    }
    Ok(seconds)
}

#[cfg(all(windows, feature = "cpal-output"))]
fn parse_boolean(value: &str, name: &str) -> Result<bool, std::io::Error> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => invalid_input(name, "must be true or false"),
    }
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

#[cfg(all(test, windows, feature = "cpal-output"))]
mod tests {
    use super::*;

    #[test]
    fn duration_and_boolean_parser_bounds_are_explicit() {
        assert_eq!(parse_seconds("1", "wait", 600).unwrap(), 1);
        assert_eq!(parse_seconds("600", "wait", 600).unwrap(), 600);
        assert!(parse_seconds("0", "wait", 600).is_err());
        assert!(parse_seconds("601", "wait", 600).is_err());
        assert!(parse_boolean("true", "switch").unwrap());
        assert!(!parse_boolean("false", "switch").unwrap());
        assert!(parse_boolean("yes", "switch").is_err());
    }
}

#[cfg(not(all(windows, feature = "cpal-output")))]
fn main() {
    eprintln!("output_default_recovery requires Windows and --features cpal-output");
}
