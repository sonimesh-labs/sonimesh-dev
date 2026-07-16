//! Read-only output-device enumeration for the Windows CPAL feasibility path.

#[cfg(all(windows, feature = "cpal-output"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use sonimesh_platform::CpalOutput;

    let output = CpalOutput::new();
    let probes = output.probe_devices()?;
    println!("CPAL output devices: {}", probes.len());
    for probe in probes {
        let default_marker = if probe.device.is_default {
            " [default]"
        } else {
            ""
        };
        println!("- {}{}", probe.device.display_name, default_marker);
        println!("  id: {}", probe.device.id.as_str());
        println!(
            "  default: {} Hz, {} channels, {}",
            probe.sample_rate_hz, probe.channels, probe.backend_sample_format
        );
        println!("  buffer: {:?}", probe.buffer_size);
    }
    Ok(())
}

#[cfg(not(all(windows, feature = "cpal-output")))]
fn main() {
    eprintln!("output_probe requires Windows and --features cpal-output");
}
