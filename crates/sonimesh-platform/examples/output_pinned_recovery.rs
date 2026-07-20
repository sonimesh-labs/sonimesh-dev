//! Manual pinned-device CPAL output recovery experiment for Windows lab use.

#[cfg(all(windows, feature = "cpal-output"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::thread;

    use sonimesh_platform::{AudioOutput, CpalOutput, OutputDeviceId};

    let experiment = experiment_config()?;
    let mut output = CpalOutput::new();
    let pinned_id = OutputDeviceId::new(experiment.pinned_device_id);
    let initial_probe = pinned_probe(&output, &pinned_id)?;
    let initial_config = initial_probe.output_config(Some(480), 8_192)?;
    let initial_config = start_with_fixed_or_default(&mut output, &pinned_id, initial_config)?;

    println!("initial format: {}", format_label(&initial_probe));
    println!(
        "wait for device event (s): {}",
        experiment.event_wait.as_secs()
    );
    println!("pinned device identifier: redacted");
    println!(
        "initial request: {}",
        request_label(initial_config.requested_callback_frames)
    );
    thread::sleep(experiment.event_wait);

    let before_stop = output.status();
    let before_stop_health = output.health();
    print_status("before_stop", before_stop, before_stop_health);
    output.stop();

    let recovered_probe = match pinned_probe(&output, &pinned_id) {
        Ok(probe) => probe,
        Err(sonimesh_platform::OutputError::DeviceUnavailable) => {
            println!("recovery_result=pinned_device_unavailable");
            return Ok(());
        }
        Err(error) => return Err(error.into()),
    };
    println!("recovered format: {}", format_label(&recovered_probe));
    let recovered_config = recovered_probe.output_config(Some(480), 8_192)?;
    let recovered_config = start_with_fixed_or_default(&mut output, &pinned_id, recovered_config)?;
    println!(
        "recovered request: {}",
        request_label(recovered_config.requested_callback_frames)
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

    println!(
        "pre_stop_healthy={}",
        is_healthy(before_stop, before_stop_health)
    );
    if !is_healthy(recovered_status, recovered_health) {
        return Err("CPAL pinned-device recovery experiment did not remain healthy".into());
    }
    println!("recovery_result=pinned_device_restarted");
    Ok(())
}

#[cfg(all(windows, feature = "cpal-output"))]
fn pinned_probe(
    output: &sonimesh_platform::CpalOutput,
    pinned_id: &sonimesh_platform::OutputDeviceId,
) -> Result<sonimesh_platform::CpalDeviceProbe, sonimesh_platform::OutputError> {
    output
        .probe_devices()?
        .into_iter()
        .find(|probe| probe.device.id == *pinned_id)
        .ok_or(sonimesh_platform::OutputError::DeviceUnavailable)
}

#[cfg(all(windows, feature = "cpal-output"))]
fn start_with_fixed_or_default(
    output: &mut sonimesh_platform::CpalOutput,
    pinned_id: &sonimesh_platform::OutputDeviceId,
    fixed_config: sonimesh_platform::OutputConfig,
) -> Result<sonimesh_platform::OutputConfig, sonimesh_platform::OutputError> {
    use sonimesh_platform::{AudioOutput, OutputConfig, RenderSource};

    struct Silence;

    impl RenderSource for Silence {
        fn render(&mut self, output: &mut [f32]) {
            output.fill(0.0);
        }
    }

    match output.start(Some(pinned_id), fixed_config, Box::new(Silence)) {
        Ok(()) => Ok(fixed_config),
        Err(error) => {
            println!("fixed 480-frame request rejected: {error}");
            let default_config = OutputConfig {
                requested_callback_frames: None,
                ..fixed_config
            };
            output.start(Some(pinned_id), default_config, Box::new(Silence))?;
            Ok(default_config)
        }
    }
}

#[cfg(all(windows, feature = "cpal-output"))]
fn is_healthy(
    status: sonimesh_platform::CpalOutputStatus,
    health: sonimesh_platform::OutputHealth,
) -> bool {
    status.callback_count > 0
        && status.oversized_callbacks == 0
        && status.stream_errors == 0
        && health == sonimesh_platform::OutputHealth::Healthy
}

#[cfg(all(windows, feature = "cpal-output"))]
fn format_label(probe: &sonimesh_platform::CpalDeviceProbe) -> String {
    format!(
        "{} Hz, {} channels, {}",
        probe.sample_rate_hz, probe.channels, probe.backend_sample_format
    )
}

#[cfg(all(windows, feature = "cpal-output"))]
fn request_label(request: Option<u32>) -> String {
    request.map_or_else(|| "default".to_owned(), |frames| frames.to_string())
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
#[derive(Debug, Eq, PartialEq)]
struct ExperimentConfig {
    pinned_device_id: String,
    event_wait: std::time::Duration,
    recovery_duration: std::time::Duration,
}

#[cfg(all(windows, feature = "cpal-output"))]
fn experiment_config() -> Result<ExperimentConfig, std::io::Error> {
    Ok(ExperimentConfig {
        pinned_device_id: required_environment("SONIMESH_OUTPUT_PINNED_DEVICE_ID")?,
        event_wait: std::time::Duration::from_secs(environment_seconds(
            "SONIMESH_OUTPUT_DEVICE_EVENT_WAIT_SECONDS",
            30,
            600,
        )?),
        recovery_duration: std::time::Duration::from_secs(environment_seconds(
            "SONIMESH_OUTPUT_RECOVERY_SECONDS",
            2,
            60,
        )?),
    })
}

#[cfg(all(windows, feature = "cpal-output"))]
fn required_environment(name: &str) -> Result<String, std::io::Error> {
    match std::env::var(name) {
        Ok(value) => validate_device_id(value, name),
        Err(std::env::VarError::NotPresent) => invalid_input(name, "must be set"),
        Err(std::env::VarError::NotUnicode(_)) => invalid_input(name, "must be valid Unicode"),
    }
}

#[cfg(all(windows, feature = "cpal-output"))]
fn validate_device_id(value: String, name: &str) -> Result<String, std::io::Error> {
    if value.is_empty() {
        return invalid_input(name, "must be set");
    }
    Ok(value)
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
    fn duration_and_required_identifier_bounds_are_explicit() {
        assert_eq!(parse_seconds("1", "wait", 600).unwrap(), 1);
        assert_eq!(parse_seconds("600", "wait", 600).unwrap(), 600);
        assert!(parse_seconds("0", "wait", 600).is_err());
        assert!(parse_seconds("601", "wait", 600).is_err());
        assert_eq!(
            validate_device_id("wasapi:test".to_owned(), "device").unwrap(),
            "wasapi:test"
        );
        assert!(validate_device_id(String::new(), "device").is_err());
    }
}

#[cfg(not(all(windows, feature = "cpal-output")))]
fn main() {
    eprintln!("output_pinned_recovery requires Windows and --features cpal-output");
}
