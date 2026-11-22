use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputDevice {
    label: String,
    sample_rate_hz: u32,
    channels: u16,
    is_default: bool,
}

#[tauri::command]
fn probe_outputs() -> Result<Vec<OutputDevice>, &'static str> {
    #[cfg(windows)]
    {
        let probes = sonimesh_platform::CpalOutput::new()
            .probe_devices()
            .map_err(|_| "Could not inspect output devices.")?;
        Ok(probes
            .into_iter()
            .map(|probe| OutputDevice {
                label: probe.device.display_name,
                sample_rate_hz: probe.sample_rate_hz,
                channels: probe.channels,
                is_default: probe.device.is_default,
            })
            .collect())
    }
    #[cfg(not(windows))]
    Err("Output probing is available only on Windows.")
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![probe_outputs])
        .run(tauri::generate_context!())
        .expect("desktop output probe failed");
}
