use std::sync::Mutex;

use serde::Serialize;
use sonimesh_platform::{CpalDeviceProbe, CpalOutput};

#[derive(Default)]
struct ShellState {
    output: CpalOutput,
    selected_output_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopSnapshot {
    session: &'static str,
    output_devices: Vec<OutputDevice>,
    selected_output_id: Option<String>,
    gain: f32,
    muted: bool,
    output: OutputState,
    pairing: PairingState,
    reconnect_remaining_ms: Option<u32>,
    metrics: StreamMetrics,
    replies: Vec<ControlReply>,
    diagnostic_exports: Vec<DiagnosticExportReceipt>,
    capabilities: DesktopCapabilities,
    preview_adapter: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlReply {
    kind: &'static str,
    request_id: u64,
    detail: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticExportReceipt {
    sequence: String,
    bytes: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopCapabilities {
    receiver: bool,
    pairing: bool,
    stream_controls: bool,
    diagnostics: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputDevice {
    id: String,
    label: String,
    sample_rate_hz: u32,
    channels: u16,
    is_default: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputState {
    selection_retained: bool,
    stream_active: bool,
    fault_reported: bool,
    health: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairingState {
    state: &'static str,
    expires_in_seconds: Option<u32>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StreamMetrics {
    authenticated_packets: String,
    duplicate_packets: String,
    rejected_packets: String,
    late_packets: String,
    reordered_packets: String,
    jitter_resync_dropped_frames: String,
    plc_frames: String,
    decode_errors: String,
    pcm_underflows: String,
    pcm_refill_limit_hits: String,
    pcm_overflows: String,
    pcm_discontinuities: String,
    queued_pcm_blocks: u32,
    queued_pcm_high_water_blocks: u32,
    jitter_frames: u32,
    jitter_high_water_frames: u32,
    target_min_frames: u32,
    target_max_frames: u32,
    target_frames: u32,
    target_smoothed_jitter_ms: u32,
    target_raised_steps: u32,
    target_lowered_steps: u32,
    target_gated_ticks: String,
    target_forced_playouts: String,
    stream_resets: String,
    nominal_input_sample_rate_hz: u32,
    nominal_output_sample_rate_hz: u32,
    drift_adjustment_ppm: i32,
    drift_hard_resyncs: String,
    current_jitter_ms: u32,
    playout_delay_ms: u32,
}

impl From<CpalDeviceProbe> for OutputDevice {
    fn from(probe: CpalDeviceProbe) -> Self {
        Self {
            id: probe.device.id.as_str().to_owned(),
            label: probe.device.display_name,
            sample_rate_hz: probe.sample_rate_hz,
            channels: probe.channels,
            is_default: probe.device.is_default,
        }
    }
}

impl DesktopSnapshot {
    fn from_state(state: &mut ShellState) -> Result<Self, String> {
        let output_devices = state
            .output
            .probe_devices()
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(OutputDevice::from)
            .collect::<Vec<_>>();
        state.selected_output_id =
            retain_available_output(state.selected_output_id.as_deref(), &output_devices);

        Ok(Self {
            session: "idle",
            output_devices,
            selected_output_id: state.selected_output_id.clone(),
            gain: 1.0,
            muted: false,
            output: OutputState {
                selection_retained: state.selected_output_id.is_some(),
                stream_active: false,
                fault_reported: false,
                health: "healthy",
            },
            pairing: PairingState {
                state: "inactive",
                expires_in_seconds: None,
            },
            reconnect_remaining_ms: None,
            metrics: StreamMetrics::empty(),
            replies: Vec::new(),
            diagnostic_exports: Vec::new(),
            capabilities: DesktopCapabilities {
                receiver: false,
                pairing: false,
                stream_controls: false,
                diagnostics: false,
            },
            preview_adapter: false,
        })
    }
}

impl StreamMetrics {
    fn empty() -> Self {
        Self {
            authenticated_packets: "0".into(),
            duplicate_packets: "0".into(),
            rejected_packets: "0".into(),
            late_packets: "0".into(),
            reordered_packets: "0".into(),
            jitter_resync_dropped_frames: "0".into(),
            plc_frames: "0".into(),
            decode_errors: "0".into(),
            pcm_underflows: "0".into(),
            pcm_refill_limit_hits: "0".into(),
            pcm_overflows: "0".into(),
            pcm_discontinuities: "0".into(),
            queued_pcm_blocks: 0,
            queued_pcm_high_water_blocks: 0,
            jitter_frames: 0,
            jitter_high_water_frames: 0,
            target_min_frames: 0,
            target_max_frames: 0,
            target_frames: 0,
            target_smoothed_jitter_ms: 0,
            target_raised_steps: 0,
            target_lowered_steps: 0,
            target_gated_ticks: "0".into(),
            target_forced_playouts: "0".into(),
            stream_resets: "0".into(),
            nominal_input_sample_rate_hz: 0,
            nominal_output_sample_rate_hz: 0,
            drift_adjustment_ppm: 0,
            drift_hard_resyncs: "0".into(),
            current_jitter_ms: 0,
            playout_delay_ms: 0,
        }
    }
}

#[tauri::command]
fn desktop_snapshot(state: tauri::State<'_, Mutex<ShellState>>) -> Result<DesktopSnapshot, String> {
    let mut state = state.lock().map_err(|_| "Desktop state is unavailable.")?;
    DesktopSnapshot::from_state(&mut state)
}

#[tauri::command]
fn desktop_select_output(
    state: tauri::State<'_, Mutex<ShellState>>,
    device_id: String,
) -> Result<DesktopSnapshot, String> {
    let mut state = state.lock().map_err(|_| "Desktop state is unavailable.")?;
    let available = state
        .output
        .probe_devices()
        .map_err(|error| error.to_string())?;
    let inventory = available
        .into_iter()
        .map(OutputDevice::from)
        .collect::<Vec<_>>();
    state.selected_output_id = Some(select_available_output(&device_id, &inventory)?);
    DesktopSnapshot::from_state(&mut state)
}

fn select_available_output(device_id: &str, devices: &[OutputDevice]) -> Result<String, String> {
    devices
        .iter()
        .find(|device| device.id == device_id)
        .map(|device| device.id.clone())
        .ok_or_else(|| "The selected output device is no longer available.".into())
}

fn retain_available_output(selected_id: Option<&str>, devices: &[OutputDevice]) -> Option<String> {
    selected_id
        .and_then(|selected_id| devices.iter().find(|device| device.id == selected_id))
        .or_else(|| devices.iter().find(|device| device.is_default))
        .or_else(|| devices.first())
        .map(|device| device.id.clone())
}

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(ShellState::default()))
        .invoke_handler(tauri::generate_handler![
            desktop_snapshot,
            desktop_select_output
        ])
        .run(tauri::generate_context!())
        .expect("failed to run SoniMesh desktop shell");
}

#[cfg(test)]
mod tests {
    use super::{
        DesktopCapabilities, DesktopSnapshot, OutputDevice, retain_available_output,
        select_available_output,
    };

    #[test]
    fn output_selection_requires_an_id_from_the_current_inventory() {
        let devices = vec![OutputDevice {
            id: "device-a".into(),
            label: "Speakers".into(),
            sample_rate_hz: 48_000,
            channels: 2,
            is_default: true,
        }];

        assert_eq!(
            select_available_output("device-a", &devices).unwrap(),
            "device-a"
        );
        assert!(select_available_output("removed-device", &devices).is_err());
    }

    #[test]
    fn refreshing_inventory_falls_back_when_selected_output_disappears() {
        let devices = vec![
            OutputDevice {
                id: "default-device".into(),
                label: "Speakers".into(),
                sample_rate_hz: 48_000,
                channels: 2,
                is_default: true,
            },
            OutputDevice {
                id: "other-device".into(),
                label: "Headphones".into(),
                sample_rate_hz: 48_000,
                channels: 2,
                is_default: false,
            },
        ];

        assert_eq!(
            retain_available_output(Some("removed-device"), &devices).as_deref(),
            Some("default-device")
        );
        assert_eq!(
            retain_available_output(Some("other-device"), &devices).as_deref(),
            Some("other-device")
        );
        assert_eq!(retain_available_output(None, &[]), None);
    }

    #[test]
    fn output_setup_snapshot_marks_unimplemented_receiver_controls_unavailable() {
        let capabilities = DesktopCapabilities {
            receiver: false,
            pairing: false,
            stream_controls: false,
            diagnostics: false,
        };
        let encoded = serde_json::to_value(capabilities).unwrap();

        assert_eq!(encoded["receiver"], false);
        assert_eq!(encoded["pairing"], false);
        assert_eq!(encoded["streamControls"], false);
        assert_eq!(encoded["diagnostics"], false);
    }

    #[test]
    fn snapshot_contract_uses_frontend_camel_case_names() {
        let snapshot = DesktopSnapshot {
            session: "idle",
            output_devices: Vec::new(),
            selected_output_id: None,
            gain: 1.0,
            muted: false,
            output: super::OutputState {
                selection_retained: false,
                stream_active: false,
                fault_reported: false,
                health: "healthy",
            },
            pairing: super::PairingState {
                state: "inactive",
                expires_in_seconds: None,
            },
            reconnect_remaining_ms: None,
            metrics: super::StreamMetrics::empty(),
            replies: Vec::new(),
            diagnostic_exports: Vec::new(),
            capabilities: DesktopCapabilities {
                receiver: false,
                pairing: false,
                stream_controls: false,
                diagnostics: false,
            },
            preview_adapter: false,
        };
        let encoded = serde_json::to_value(snapshot).unwrap();

        assert_eq!(encoded["outputDevices"].as_array().unwrap().len(), 0);
        assert_eq!(encoded["selectedOutputId"], serde_json::Value::Null);
        assert_eq!(encoded["previewAdapter"], false);
        assert!(encoded.get("output_devices").is_none());
        assert_eq!(encoded["metrics"]["authenticatedPackets"], "0");
    }
}
