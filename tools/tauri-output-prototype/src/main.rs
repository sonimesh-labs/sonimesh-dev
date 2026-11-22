use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputDevice {
    label: String,
    sample_rate_hz: Option<u32>,
    channels: Option<u16>,
    is_default: bool,
}

#[tauri::command]
fn probe_outputs() -> Result<Vec<OutputDevice>, &'static str> {
    #[cfg(windows)]
    {
        use sonimesh_platform::AudioOutput;

        let output = sonimesh_platform::CpalOutput::new();
        let devices = output
            .devices()
            .map_err(|_| "Could not inspect output devices.")?;
        Ok(map_devices(devices, |id| {
            output
                .device_probe(id)
                .ok()
                .map(|probe| (probe.sample_rate_hz, probe.channels))
        }))
    }
    #[cfg(not(windows))]
    Err("Output probing is available only on Windows.")
}

#[cfg(windows)]
fn map_devices(
    devices: Vec<sonimesh_platform::OutputDeviceInfo>,
    mut format: impl FnMut(&sonimesh_platform::OutputDeviceId) -> Option<(u32, u16)>,
) -> Vec<OutputDevice> {
    devices
        .into_iter()
        .map(|device| {
            let (sample_rate_hz, channels) = format(&device.id)
                .map(|(rate, channels)| (Some(rate), Some(channels)))
                .unwrap_or((None, None));
            OutputDevice {
                label: device.display_name,
                sample_rate_hz,
                channels,
                is_default: device.is_default,
            }
        })
        .collect()
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use sonimesh_platform::{OutputDeviceId, OutputDeviceInfo};

    #[test]
    fn unavailable_format_does_not_hide_other_endpoints() {
        let devices = ["unavailable", "working"].map(|name| OutputDeviceInfo {
            id: OutputDeviceId::new(name),
            display_name: name.to_owned(),
            is_default: name == "working",
        });
        let mapped = map_devices(devices.into(), |id| {
            (id.as_str() == "working").then_some((48_000, 2))
        });
        assert_eq!(mapped.len(), 2);
        assert_eq!(mapped[0].label, "unavailable");
        assert_eq!(mapped[0].sample_rate_hz, None);
        assert_eq!(mapped[0].channels, None);
        assert_eq!(mapped[1].sample_rate_hz, Some(48_000));
        assert_eq!(mapped[1].channels, Some(2));
        assert!(mapped[1].is_default);
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![probe_outputs])
        .run(tauri::generate_context!())
        .expect("desktop output probe failed");
}
