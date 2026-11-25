use serde::Serialize;
mod lab;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
struct ProbeState {
    snapshot: Mutex<SelectionSnapshot>,
    playing: Arc<AtomicBool>,
    lab: Arc<Mutex<lab::Lab>>,
}

fn spawn_lab_monitor(lab: &Arc<Mutex<lab::Lab>>) -> std::thread::JoinHandle<()> {
    let lab = Arc::downgrade(lab);
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let Some(lab) = lab.upgrade() else { break };
            let Ok(mut lab) = lab.lock() else { break };
            lab.refresh();
        }
    })
}

#[tauri::command]
fn suggest_lab_host(android_host: &str) -> Result<String, &'static str> {
    use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};

    let target: Ipv4Addr = android_host
        .parse()
        .map_err(|_| "Enter the Android device's IPv4 address.")?;
    if target.is_unspecified()
        || target.is_multicast()
        || target.is_broadcast()
        || target.is_loopback()
    {
        return Err("Enter a non-loopback unicast Android IPv4 address.");
    }
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
        .map_err(|_| "Could not inspect the network route.")?;
    socket
        .connect(SocketAddrV4::new(target, 39093))
        .map_err(|_| "No network route to that address. Enter the desktop address manually.")?;
    match socket.local_addr() {
        Ok(std::net::SocketAddr::V4(local))
            if !local.ip().is_unspecified() && !local.ip().is_loopback() =>
        {
            Ok(local.ip().to_string())
        }
        _ => Err("Could not determine a desktop IPv4 address. Enter it manually."),
    }
}

#[cfg(test)]
mod route_tests {
    use super::{ProbeState, spawn_lab_monitor, suggest_lab_host};

    #[test]
    fn listener_output_is_drained_without_ui_polling() {
        use std::time::{Duration, Instant};
        let state = ProbeState::default();
        let monitor = spawn_lab_monitor(&state.lab);
        state.lab.lock().unwrap().test_output_burst();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if state.lab.lock().unwrap().test_volume() == Some(42) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "background output drain timed out"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(state);
        monitor.join().unwrap();
    }

    #[test]
    #[ignore = "requires the playback lab binary and free lab ports"]
    fn real_listener_output_is_drained_without_ui_polling() {
        use std::io::Write;
        use std::time::{Duration, Instant};

        let state = ProbeState::default();
        state
            .start_listener(|lab| lab.start("127.0.0.1", None, 60, false, 39094, 47610))
            .unwrap();
        let monitor = spawn_lab_monitor(&state.lab);
        let mut stdin = state.lab.lock().unwrap().test_take_stdin();
        let writer = std::thread::spawn(move || {
            stdin
                .write_all("volume 1\n".repeat(512).as_bytes())
                .unwrap();
            stdin.write_all(b"volume 42\n").unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if state.lab.lock().unwrap().test_volume() == Some(42) {
                break;
            }
            assert!(Instant::now() < deadline, "listener output drain timed out");
            std::thread::sleep(Duration::from_millis(20));
        }
        writer.join().unwrap();
        state.lab.lock().unwrap().stop();
        drop(state);
        monitor.join().unwrap();
    }

    #[test]
    fn rejects_addresses_that_cannot_identify_an_android_peer() {
        for address in [
            "",
            "invalid",
            "0.0.0.0",
            "127.0.0.1",
            "224.0.0.1",
            "255.255.255.255",
            "::1",
        ] {
            assert!(suggest_lab_host(address).is_err(), "{address}");
        }
    }

    #[test]
    fn test_tone_reservation_blocks_listener_until_guard_is_dropped() {
        let state = ProbeState::default();
        let guard = state.reserve_tone().unwrap();
        let result = state.start_listener(|_| -> Result<(), String> {
            panic!("listener must not start while the test tone is reserved")
        });
        assert_eq!(
            result.unwrap_err(),
            "Wait for the test tone to finish before starting the listener."
        );
        assert!(state.reserve_tone().is_err());
        drop(guard);
        assert!(state.start_listener(|_| Ok(())).is_ok());
    }

    #[test]
    fn listener_start_and_tone_reservation_are_serialized() {
        use std::sync::{Arc, mpsc};
        let state = Arc::new(ProbeState::default());
        let worker_state = Arc::clone(&state);
        let (started, started_rx) = mpsc::channel();
        let (finish, finish_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            worker_state
                .start_listener(|lab| {
                    started.send(()).unwrap();
                    finish_rx.recv().unwrap();
                    lab.simulate_running();
                    Ok(())
                })
                .unwrap();
        });
        started_rx.recv().unwrap();
        let tone_state = Arc::clone(&state);
        let tone = std::thread::spawn(move || tone_state.reserve_tone().is_err());
        finish.send(()).unwrap();
        worker.join().unwrap();
        assert!(tone.join().unwrap());
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Independent listener and output selections from the probe.
fn start_lab(
    state: tauri::State<'_, ProbeState>,
    host: String,
    generation: u32,
    index: Option<usize>,
    startup_buffer_ms: u16,
    adaptive: bool,
    media_port: u16,
    control_port: u16,
) -> Result<lab::LabSnapshot, String> {
    let output = match index {
        Some(index) => Some(
            state
                .snapshot
                .lock()
                .map_err(|_| "Could not select output.")?
                .select(generation, index)?,
        ),
        None => None,
    };
    state.start_listener(|lab| {
        lab.start(
            &host,
            output.as_ref().map(|id| id.as_str()),
            startup_buffer_ms,
            adaptive,
            media_port,
            control_port,
        )
    })
}

impl ProbeState {
    fn start_listener<T>(
        &self,
        start: impl FnOnce(&mut lab::Lab) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut lab = self
            .lab
            .lock()
            .map_err(|_| "Could not access listener state.")?;
        if self.playing.load(Ordering::Acquire) {
            return Err("Wait for the test tone to finish before starting the listener.".into());
        }
        start(&mut lab)
    }

    fn reserve_tone(&self) -> Result<PlaybackGuard, &'static str> {
        let mut lab = self
            .lab
            .lock()
            .map_err(|_| "Could not access listener state.")?;
        if lab.refresh().running {
            return Err("Stop the listener before testing an output.");
        }
        self.playing
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| "A test tone is already playing.")?;
        Ok(PlaybackGuard(Arc::clone(&self.playing)))
    }
}

#[tauri::command]
fn lab_status(state: tauri::State<'_, ProbeState>) -> Result<lab::LabSnapshot, &'static str> {
    Ok(state
        .lab
        .lock()
        .map_err(|_| "Could not access listener state.")?
        .refresh())
}

#[tauri::command]
fn stop_lab(state: tauri::State<'_, ProbeState>) -> Result<lab::LabSnapshot, &'static str> {
    Ok(state
        .lab
        .lock()
        .map_err(|_| "Could not access listener state.")?
        .stop())
}

#[tauri::command]
fn control_lab(
    state: tauri::State<'_, ProbeState>,
    volume: Option<u8>,
    muted: Option<bool>,
) -> Result<(), &'static str> {
    state
        .lab
        .lock()
        .map_err(|_| "Could not access listener state.")?
        .control(volume, muted)
}

#[derive(Default)]
struct SelectionSnapshot {
    generation: u32,
    ids: Vec<sonimesh_platform::OutputDeviceId>,
}

impl SelectionSnapshot {
    fn select(
        &self,
        generation: u32,
        index: usize,
    ) -> Result<sonimesh_platform::OutputDeviceId, &'static str> {
        if self.generation != generation {
            return Err("Device list changed. Refresh and try again.");
        }
        self.ids
            .get(index)
            .cloned()
            .ok_or("Device is no longer available.")
    }

    fn matching_index(
        &self,
        generation: u32,
        index: Option<usize>,
        ids: &[sonimesh_platform::OutputDeviceId],
    ) -> Option<usize> {
        let id = self.select(generation, index?).ok()?;
        if self
            .ids
            .iter()
            .filter(|candidate| **candidate == id)
            .count()
            != 1
        {
            return None;
        }
        let mut matches = ids
            .iter()
            .enumerate()
            .filter(|(_, candidate)| **candidate == id);
        let (position, _) = matches.next()?;
        if matches.next().is_some() {
            None
        } else {
            Some(position)
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputSnapshot {
    generation: u32,
    devices: Vec<OutputDevice>,
    selected_index: Option<usize>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutputDevice {
    label: String,
    sample_rate_hz: Option<u32>,
    channels: Option<u16>,
    is_default: bool,
}

#[tauri::command]
fn probe_outputs(
    state: tauri::State<'_, ProbeState>,
    generation: u32,
    selected_index: Option<usize>,
) -> Result<OutputSnapshot, &'static str> {
    #[cfg(windows)]
    {
        use sonimesh_platform::AudioOutput;

        let output = sonimesh_platform::CpalOutput::new();
        let devices = output
            .devices()
            .map_err(|_| "Could not inspect output devices.")?;
        let ids: Vec<_> = devices.iter().map(|device| device.id.clone()).collect();
        let devices = map_devices(devices, |id| {
            output
                .device_probe(id)
                .ok()
                .map(|probe| (probe.sample_rate_hz, probe.channels))
        });
        let mut snapshot = state
            .snapshot
            .lock()
            .map_err(|_| "Could not inspect output devices.")?;
        let selected_index = snapshot.matching_index(generation, selected_index, &ids);
        snapshot.generation = snapshot
            .generation
            .checked_add(1)
            .ok_or("Refresh limit reached. Restart the probe.")?;
        snapshot.ids = ids;
        Ok(OutputSnapshot {
            generation: snapshot.generation,
            devices,
            selected_index,
        })
    }
    #[cfg(not(windows))]
    Err("Output probing is available only on Windows.")
}

struct PlaybackGuard(Arc<AtomicBool>);

impl Drop for PlaybackGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[tauri::command]
async fn play_test_tone(
    state: tauri::State<'_, ProbeState>,
    generation: u32,
    index: usize,
) -> Result<(), &'static str> {
    #[cfg(windows)]
    {
        let id = {
            let snapshot = state
                .snapshot
                .lock()
                .map_err(|_| "Could not select output.")?;
            snapshot.select(generation, index)?
        };
        let guard = state.reserve_tone()?;
        tauri::async_runtime::spawn_blocking(move || {
            let _guard = guard;
            play_tone(&id)
        })
        .await
        .map_err(|_| "Could not play test tone.")?
    }
    #[cfg(not(windows))]
    {
        let _ = (state, generation, index);
        Err("Output testing is available only on Windows.")
    }
}

#[cfg(windows)]
fn play_tone(id: &sonimesh_platform::OutputDeviceId) -> Result<(), &'static str> {
    use sonimesh_platform::{AudioOutput, CpalOutput, MAX_CALLBACK_FRAMES};
    use std::time::Duration;
    let mut output = CpalOutput::new();
    let probe = output
        .device_probe(id)
        .map_err(|_| "Could not inspect this output.")?;
    let config = probe
        .output_config(None, MAX_CALLBACK_FRAMES)
        .map_err(|_| "This output format is not supported.")?;
    let completed = Arc::new(AtomicBool::new(false));
    output
        .start(
            Some(id),
            config,
            Box::new(TestTone::new(
                config.sample_rate_hz,
                config.channels,
                Arc::clone(&completed),
            )),
        )
        .map_err(|_| "Could not start this output.")?;
    let generated = wait_for_tone(&completed, Duration::from_secs(5));
    if generated {
        let observed = output.status().maximum_callback_frames;
        std::thread::sleep(drain_delay(observed, config.sample_rate_hz));
    }
    let status = output.status();
    output.stop();
    if !generated
        || status.callback_count == 0
        || status.stream_errors > 0
        || status.oversized_callbacks > 0
    {
        return Err("Output stopped or reported a playback error.");
    }
    Ok(())
}

#[cfg(windows)]
fn drain_delay(observed_frames: u32, sample_rate_hz: u32) -> std::time::Duration {
    std::time::Duration::from_secs_f64(f64::from(observed_frames) / f64::from(sample_rate_hz))
        + std::time::Duration::from_millis(200)
}

#[cfg(windows)]
fn wait_for_tone(completed: &AtomicBool, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while !completed.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    completed.load(Ordering::Acquire)
}

#[cfg(windows)]
struct TestTone {
    rate: u32,
    channels: usize,
    frame: u32,
    completed: Arc<AtomicBool>,
}

#[cfg(windows)]
impl TestTone {
    fn new(rate: u32, channels: u16, completed: Arc<AtomicBool>) -> Self {
        Self {
            rate,
            channels: usize::from(channels),
            frame: 0,
            completed,
        }
    }
}

#[cfg(windows)]
impl sonimesh_platform::RenderSource for TestTone {
    fn render(&mut self, output: &mut [f32]) {
        let total = self.rate * 2;
        let fade = (self.rate / 100).max(1);
        for samples in output.chunks_mut(self.channels) {
            let value = if self.frame < total {
                let gain = (self.frame.min(total - self.frame) as f32 / fade as f32).min(1.0);
                let phase = std::f32::consts::TAU * 440.0 * (self.frame % self.rate) as f32
                    / self.rate as f32;
                self.frame += 1;
                phase.sin() * 0.04 * gain
            } else {
                0.0
            };
            samples.fill(value);
        }
        if self.frame >= total {
            self.completed.store(true, Ordering::Release);
        }
    }
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
    use sonimesh_platform::{OutputDeviceId, OutputDeviceInfo, RenderSource};

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

    #[test]
    fn selection_rejects_an_old_snapshot_and_missing_entry() {
        let snapshot = SelectionSnapshot {
            generation: 5,
            ids: vec![OutputDeviceId::new("first")],
        };
        assert_eq!(snapshot.select(5, 0).unwrap().as_str(), "first");
        assert!(snapshot.select(4, 0).is_err());
        assert!(snapshot.select(5, 1).is_err());
    }

    #[test]
    fn refresh_matches_only_the_original_unique_endpoint() {
        let snapshot = SelectionSnapshot {
            generation: 5,
            ids: vec![OutputDeviceId::new("first"), OutputDeviceId::new("chosen")],
        };
        let reordered = vec![OutputDeviceId::new("chosen"), OutputDeviceId::new("first")];
        assert_eq!(snapshot.matching_index(5, Some(1), &reordered), Some(0));
        assert_eq!(snapshot.matching_index(4, Some(1), &reordered), None);
        assert_eq!(snapshot.matching_index(5, Some(2), &reordered), None);
        assert_eq!(snapshot.matching_index(5, None, &reordered), None);
        assert_eq!(snapshot.matching_index(5, Some(1), &reordered[1..]), None);
        assert_eq!(
            snapshot.matching_index(
                5,
                Some(1),
                &[OutputDeviceId::new("chosen"), OutputDeviceId::new("chosen")]
            ),
            None
        );
        let ambiguous = SelectionSnapshot {
            generation: 6,
            ids: vec![OutputDeviceId::new("chosen"), OutputDeviceId::new("chosen")],
        };
        assert_eq!(ambiguous.matching_index(6, Some(0), &reordered), None);
    }

    #[test]
    fn waits_for_completion_and_rejects_a_stalled_output() {
        use std::time::Duration;
        let completed = Arc::new(AtomicBool::new(false));
        assert!(!wait_for_tone(&completed, Duration::from_millis(1)));
        let worker_flag = Arc::clone(&completed);
        let worker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            worker_flag.store(true, Ordering::Release);
        });
        assert!(wait_for_tone(&completed, Duration::from_secs(1)));
        worker.join().unwrap();
    }

    #[test]
    fn drain_delay_covers_the_largest_observed_callback() {
        use std::time::Duration;
        assert_eq!(drain_delay(8_000, 8_000), Duration::from_millis(1_200));
        assert_eq!(drain_delay(480, 48_000), Duration::from_millis(210));
    }

    #[test]
    fn tone_is_bounded_continuous_across_buffers_and_ends_in_silence() {
        let completed = Arc::new(AtomicBool::new(false));
        let mut tone = TestTone::new(48_000, 2, Arc::clone(&completed));
        let mut first = [0.0; 960];
        tone.render(&mut first);
        assert!(!completed.load(Ordering::Acquire));
        assert_eq!(first[0], 0.0);
        assert!(first.iter().all(|sample| sample.abs() <= 0.04));
        assert!(
            first
                .as_chunks::<2>()
                .0
                .iter()
                .all(|frame| frame[0] == frame[1])
        );
        let mut rest = vec![0.0; 48_000 * 4];
        tone.render(&mut rest);
        assert!(completed.load(Ordering::Acquire));
        assert!(rest[..1000].iter().any(|sample| *sample != 0.0));
        assert!(rest[rest.len() - 100..].iter().all(|sample| *sample == 0.0));
    }
}

fn main() {
    use tauri::Manager;
    let state = ProbeState::default();
    let _monitor = spawn_lab_monitor(&state.lab);
    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            probe_outputs,
            play_test_tone,
            suggest_lab_host,
            start_lab,
            lab_status,
            stop_lab,
            control_lab
        ])
        .build(tauri::generate_context!())
        .expect("desktop output probe failed")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit)
                && let Ok(mut lab) = app.state::<ProbeState>().lab.lock()
            {
                lab.stop();
            }
        });
}
