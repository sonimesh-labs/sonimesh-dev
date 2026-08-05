use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use sonimesh_platform::{
    AudioOutput, OutputConfig, OutputDeviceId, OutputDeviceInfo, OutputError, OutputHealth,
    RenderSource, SampleFormat,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    IAudioClient, IAudioRenderClient, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    WAVE_FORMAT_PCM, WAVEFORMATEX, WAVEFORMATEXTENSIBLE, eConsole, eRender,
};
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::WAVE_FORMAT_IEEE_FLOAT;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::core::{Error as WindowsError, GUID, PCWSTR};

use crate::WasapiOutputStatus;
use crate::status::WasapiStatus;

const HNS_PER_SECOND: i64 = 10_000_000;
const WAIT_TIMEOUT_MS: u32 = 250;
const IEEE_FLOAT_SUBTYPE: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const PCM_SUBTYPE: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);

/// Independent direct-WASAPI shared-mode output adapter for feasibility tests.
///
/// The adapter owns an event-driven worker. It is intentionally not linked by
/// the desktop application and must not be treated as a selected product backend.
pub struct WasapiOutput {
    worker: Option<Worker>,
    status: Arc<WasapiStatus>,
}

impl WasapiOutput {
    /// Creates a stopped direct-WASAPI feasibility adapter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            worker: None,
            status: Arc::new(WasapiStatus::default()),
        }
    }

    /// Returns a non-transactional low-rate worker status snapshot.
    #[must_use]
    pub fn status(&self) -> WasapiOutputStatus {
        self.status.snapshot()
    }

    /// Returns whether the adapter currently owns a render worker.
    #[must_use]
    pub const fn is_running(&self) -> bool {
        self.worker.is_some()
    }

    /// Builds a bounded configuration from the current default endpoint mix format.
    ///
    /// The direct prototype opens the endpoint only when this exact mix format
    /// still applies at stream start; it never uses WASAPI conversion as a
    /// substitute for an explicit SoniMesh format adapter.
    ///
    /// # Errors
    ///
    /// Returns an output error when the current default endpoint, its mix
    /// format, or the requested SoniMesh bounds cannot be read or validated.
    pub fn default_output_config(
        &self,
        requested_callback_frames: Option<u32>,
        maximum_callback_frames: u32,
    ) -> Result<OutputConfig, OutputError> {
        run_in_mta(move || {
            let mix = default_mix_format()?;
            output_config_from_mix(mix, requested_callback_frames, maximum_callback_frames)
        })
    }

    fn stop_worker(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        worker.control.request_stop();
        let _ = worker.join.join();
        self.status = Arc::new(WasapiStatus::default());
    }
}

impl Default for WasapiOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for WasapiOutput {
    fn drop(&mut self) {
        self.stop_worker();
    }
}

impl AudioOutput for WasapiOutput {
    fn devices(&self) -> Result<Vec<OutputDeviceInfo>, OutputError> {
        run_in_mta(|| Ok(vec![default_device_info()?]))
    }

    fn start(
        &mut self,
        device: Option<&OutputDeviceId>,
        config: OutputConfig,
        source: Box<dyn RenderSource>,
    ) -> Result<(), OutputError> {
        if self.worker.is_some() {
            return Err(OutputError::AlreadyRunning);
        }
        config.validate().map_err(OutputError::InvalidConfig)?;

        let requested_id = device.map(|value| value.as_str().to_owned());
        let status = Arc::new(WasapiStatus::default());
        let control = Arc::new(WorkerControl::default());
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let worker_status = Arc::clone(&status);
        let worker_control = Arc::clone(&control);
        let join = thread::Builder::new()
            .name("sonimesh-wasapi-render".to_owned())
            .spawn(move || {
                run_worker(
                    requested_id.as_deref(),
                    config,
                    source,
                    worker_status.as_ref(),
                    worker_control,
                    &ready_tx,
                );
            })
            .map_err(|error| OutputError::Backend(error.to_string()))?;

        match ready_rx.recv() {
            Ok(Ok(())) => {
                self.status = status;
                self.worker = Some(Worker { control, join });
                Ok(())
            }
            Ok(Err(error)) => {
                let _ = join.join();
                Err(error)
            }
            Err(_) => {
                let _ = join.join();
                Err(OutputError::Backend(
                    "direct WASAPI worker exited before stream startup".to_owned(),
                ))
            }
        }
    }

    fn stop(&mut self) {
        self.stop_worker();
    }

    fn health(&self) -> OutputHealth {
        let status = self.status();
        if status.stream_errors > 0 {
            OutputHealth::StreamError
        } else if status.oversized_renders > 0 {
            OutputHealth::CallbackCapacityExceeded
        } else {
            OutputHealth::Healthy
        }
    }
}

struct Worker {
    control: Arc<WorkerControl>,
    join: JoinHandle<()>,
}

#[derive(Default)]
struct WorkerControl {
    stop_requested: AtomicBool,
    wake_handle: AtomicPtr<c_void>,
}

impl WorkerControl {
    fn register_wake_handle(&self, handle: HANDLE) {
        self.wake_handle.store(handle.0, Ordering::Release);
    }

    fn unregister_wake_handle(&self, handle: HANDLE) {
        let _ = self.wake_handle.compare_exchange(
            handle.0,
            ptr::null_mut(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
        let handle = self.wake_handle.load(Ordering::Acquire);
        if !handle.is_null() {
            // The worker owns the handle lifetime and removes it before close.
            let _ = unsafe { SetEvent(HANDLE(handle)) };
        }
    }

    fn stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> Result<Self, OutputError> {
        // The direct tool confines COM to short-lived control threads or its
        // dedicated render worker, so MTA initialization cannot leak into app UI.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok() }
            .map_err(|error| backend_error(&error))?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        // Paired with a successful CoInitializeEx call in initialize.
        unsafe { CoUninitialize() };
    }
}

struct RegisteredEvent {
    handle: HANDLE,
    control: Arc<WorkerControl>,
}

impl RegisteredEvent {
    fn create(control: Arc<WorkerControl>) -> Result<Self, OutputError> {
        // Anonymous auto-reset event is owned solely by the render worker.
        let handle = unsafe { CreateEventW(None, false, false, PCWSTR::null()) }
            .map_err(|error| backend_error(&error))?;
        control.register_wake_handle(handle);
        Ok(Self { handle, control })
    }
}

impl Drop for RegisteredEvent {
    fn drop(&mut self) {
        self.control.unregister_wake_handle(self.handle);
        // The audio client is dropped before this field, so it no longer uses the event.
        let _ = unsafe { CloseHandle(self.handle) };
    }
}

struct WaveFormatAllocation(*mut WAVEFORMATEX);

impl WaveFormatAllocation {
    fn as_ptr(&self) -> *const WAVEFORMATEX {
        self.0.cast_const()
    }
}

impl Drop for WaveFormatAllocation {
    fn drop(&mut self) {
        // IAudioClient::GetMixFormat documents a CoTaskMemAlloc allocation.
        unsafe { CoTaskMemFree(Some(self.0.cast())) };
    }
}

struct RenderStream {
    client: IAudioClient,
    renderer: IAudioRenderClient,
    endpoint_buffer_frames: u32,
    mix: MixFormat,
    scratch: Vec<f32>,
    event: RegisteredEvent,
    _apartment: ComApartment,
}

impl RenderStream {
    fn prime_silence(&self, status: &WasapiStatus) -> windows::core::Result<()> {
        // WASAPI requires the shared render buffer to be initialized before Start.
        unsafe {
            self.renderer.GetBuffer(self.endpoint_buffer_frames)?;
            self.renderer.ReleaseBuffer(
                self.endpoint_buffer_frames,
                AUDCLNT_BUFFERFLAGS_SILENT.0.unsigned_abs(),
            )?;
        }
        status.primed_silence(self.endpoint_buffer_frames);
        Ok(())
    }

    fn run(
        &mut self,
        source: &mut dyn RenderSource,
        config: &OutputConfig,
        status: &WasapiStatus,
        control: &WorkerControl,
    ) {
        while !control.stop_requested() {
            // This worker is the only waiter and the event is auto-reset.
            let wait = unsafe { WaitForSingleObject(self.event.handle, WAIT_TIMEOUT_MS) };
            if control.stop_requested() {
                break;
            }
            if wait == WAIT_OBJECT_0 {
                if self
                    .render_available(source, config.maximum_callback_frames, status)
                    .is_err()
                {
                    status.stream_error();
                    break;
                }
            } else if wait != WAIT_TIMEOUT {
                status.stream_error();
                break;
            }
        }

        if unsafe { self.client.Stop() }.is_err() {
            status.stream_error();
        }
    }

    #[allow(clippy::cast_ptr_alignment)]
    fn render_available(
        &mut self,
        source: &mut dyn RenderSource,
        maximum_callback_frames: u32,
        status: &WasapiStatus,
    ) -> windows::core::Result<()> {
        let padding = unsafe { self.client.GetCurrentPadding()? };
        let frames = self.endpoint_buffer_frames.saturating_sub(padding);
        if frames == 0 {
            return Ok(());
        }

        status.render(frames);
        let data = unsafe { self.renderer.GetBuffer(frames)? };
        if frames > maximum_callback_frames {
            status.oversized();
            return unsafe {
                self.renderer
                    .ReleaseBuffer(frames, AUDCLNT_BUFFERFLAGS_SILENT.0.unsigned_abs())
            };
        }

        match self.mix.sample_format {
            SampleFormat::Float32 => {
                let samples = sample_count(frames, self.mix.channels);
                // WASAPI owns a correctly aligned interleaved buffer for the negotiated mix format.
                let output = unsafe { std::slice::from_raw_parts_mut(data.cast::<f32>(), samples) };
                output.fill(0.0);
                source.render(output);
                for sample in output {
                    *sample = sanitize_float32(*sample);
                }
            }
            SampleFormat::Signed16 => {
                let samples = sample_count(frames, self.mix.channels);
                let source_output = &mut self.scratch[..samples];
                source_output.fill(0.0);
                source.render(source_output);
                // WASAPI owns a correctly aligned interleaved buffer for the negotiated mix format.
                let output = unsafe { std::slice::from_raw_parts_mut(data.cast::<i16>(), samples) };
                for (destination, sample) in output.iter_mut().zip(source_output) {
                    *destination = float_to_i16(*sample);
                }
            }
        }

        unsafe { self.renderer.ReleaseBuffer(frames, 0) }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MixFormat {
    sample_rate_hz: u32,
    channels: u16,
    sample_format: SampleFormat,
}

fn run_worker(
    requested_id: Option<&str>,
    config: OutputConfig,
    mut source: Box<dyn RenderSource>,
    status: &WasapiStatus,
    control: Arc<WorkerControl>,
    ready_tx: &mpsc::SyncSender<Result<(), OutputError>>,
) {
    let mut stream = match open_render_stream(requested_id, config, control, status) {
        Ok(stream) => stream,
        Err(error) => {
            let _ = ready_tx.send(Err(error));
            return;
        }
    };
    if let Err(error) = stream.prime_silence(status) {
        let _ = ready_tx.send(Err(backend_error(&error)));
        return;
    }
    if let Err(error) = unsafe { stream.client.Start() } {
        let _ = ready_tx.send(Err(backend_error(&error)));
        return;
    }
    if ready_tx.send(Ok(())).is_err() {
        let _ = unsafe { stream.client.Stop() };
        return;
    }

    let control = Arc::clone(&stream.event.control);
    stream.run(source.as_mut(), &config, status, control.as_ref());
}

fn open_render_stream(
    requested_id: Option<&str>,
    config: OutputConfig,
    control: Arc<WorkerControl>,
    status: &WasapiStatus,
) -> Result<RenderStream, OutputError> {
    let apartment = ComApartment::initialize()?;
    let device = default_device()?;
    let current_id = endpoint_id(&device)?;
    if requested_id.is_some_and(|requested| requested != current_id) {
        return Err(OutputError::DeviceUnavailable);
    }

    let client: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|error| backend_error(&error))?;
    let raw_format = unsafe { client.GetMixFormat() }.map_err(|error| backend_error(&error))?;
    let mix_format = WaveFormatAllocation(raw_format);
    let mix = parse_mix_format(mix_format.as_ptr())?;
    validate_mix_config(mix, config)?;
    let duration = requested_duration(config);
    unsafe {
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            duration,
            0,
            mix_format.as_ptr(),
            None,
        )
    }
    .map_err(|error| backend_error(&error))?;
    let endpoint_buffer_frames =
        unsafe { client.GetBufferSize() }.map_err(|error| backend_error(&error))?;
    let renderer = unsafe { client.GetService::<IAudioRenderClient>() }
        .map_err(|error| backend_error(&error))?;
    let scratch = vec![
        0.0_f32;
        config
            .interleaved_samples()
            .map_err(OutputError::InvalidConfig)?
    ];
    let event = RegisteredEvent::create(control)?;
    unsafe { client.SetEventHandle(event.handle) }.map_err(|error| backend_error(&error))?;
    status.endpoint_buffer(endpoint_buffer_frames);

    Ok(RenderStream {
        client,
        renderer,
        endpoint_buffer_frames,
        mix,
        scratch,
        event,
        _apartment: apartment,
    })
}

fn default_device_info() -> Result<OutputDeviceInfo, OutputError> {
    let device = default_device()?;
    let id = endpoint_id(&device)?;
    Ok(OutputDeviceInfo {
        id: OutputDeviceId::new(id),
        display_name: "Default Windows render endpoint".to_owned(),
        is_default: true,
    })
}

fn default_mix_format() -> Result<MixFormat, OutputError> {
    let device = default_device()?;
    let client: IAudioClient =
        unsafe { device.Activate(CLSCTX_ALL, None) }.map_err(|error| backend_error(&error))?;
    let raw_format = unsafe { client.GetMixFormat() }.map_err(|error| backend_error(&error))?;
    let format = WaveFormatAllocation(raw_format);
    parse_mix_format(format.as_ptr())
}

fn default_device() -> Result<IMMDevice, OutputError> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
            .map_err(|error| backend_error(&error))?;
    unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole) }
        .map_err(|_| OutputError::DeviceUnavailable)
}

fn endpoint_id(device: &IMMDevice) -> Result<String, OutputError> {
    let raw = unsafe { device.GetId() }.map_err(|error| backend_error(&error))?;
    // The returned PWSTR remains valid until the paired CoTaskMemFree call below.
    let text = unsafe { raw.to_string() }.map_err(|error| OutputError::Backend(error.to_string()));
    unsafe { CoTaskMemFree(Some(raw.0.cast())) };
    text
}

fn output_config_from_mix(
    mix: MixFormat,
    requested_callback_frames: Option<u32>,
    maximum_callback_frames: u32,
) -> Result<OutputConfig, OutputError> {
    let config = OutputConfig {
        sample_rate_hz: mix.sample_rate_hz,
        channels: mix.channels,
        sample_format: mix.sample_format,
        requested_callback_frames,
        maximum_callback_frames,
    };
    config.validate().map_err(OutputError::InvalidConfig)?;
    Ok(config)
}

fn validate_mix_config(mix: MixFormat, config: OutputConfig) -> Result<(), OutputError> {
    if mix.sample_rate_hz != config.sample_rate_hz
        || mix.channels != config.channels
        || mix.sample_format != config.sample_format
    {
        return Err(OutputError::UnsupportedFormat);
    }
    Ok(())
}

fn requested_duration(config: OutputConfig) -> i64 {
    config.requested_callback_frames.map_or(0, |frames| {
        i64::from(frames) * HNS_PER_SECOND / i64::from(config.sample_rate_hz)
    })
}

fn parse_mix_format(raw: *const WAVEFORMATEX) -> Result<MixFormat, OutputError> {
    // GetMixFormat returns a valid allocation for at least WAVEFORMATEX.
    let base = unsafe { raw.as_ref() }.ok_or(OutputError::UnsupportedFormat)?;
    let sample_format = match u32::from(base.wFormatTag) {
        WAVE_FORMAT_IEEE_FLOAT if base.wBitsPerSample == 32 => SampleFormat::Float32,
        WAVE_FORMAT_PCM if base.wBitsPerSample == 16 => SampleFormat::Signed16,
        WAVE_FORMAT_EXTENSIBLE
            if usize::from(base.cbSize)
                >= std::mem::size_of::<WAVEFORMATEXTENSIBLE>()
                    - std::mem::size_of::<WAVEFORMATEX>() =>
        {
            // cbSize proves the allocation carries the WAVEFORMATEXTENSIBLE suffix.
            let extended = unsafe { raw.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned() };
            match (extended.SubFormat, extended.Format.wBitsPerSample) {
                (IEEE_FLOAT_SUBTYPE, 32) => SampleFormat::Float32,
                (PCM_SUBTYPE, 16) => SampleFormat::Signed16,
                _ => return Err(OutputError::UnsupportedFormat),
            }
        }
        _ => return Err(OutputError::UnsupportedFormat),
    };
    let bytes_per_sample = match sample_format {
        SampleFormat::Float32 => 4_u32,
        SampleFormat::Signed16 => 2_u32,
    };
    let expected_block_align = u32::from(base.nChannels) * bytes_per_sample;
    if base.nChannels == 0 || u32::from(base.nBlockAlign) != expected_block_align {
        return Err(OutputError::UnsupportedFormat);
    }

    Ok(MixFormat {
        sample_rate_hz: base.nSamplesPerSec,
        channels: base.nChannels,
        sample_format,
    })
}

fn sample_count(frames: u32, channels: u16) -> usize {
    usize::try_from(frames)
        .unwrap_or(usize::MAX)
        .saturating_mul(usize::from(channels))
}

fn sanitize_float32(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
fn float_to_i16(sample: f32) -> i16 {
    let sample = sanitize_float32(sample);
    if sample <= -1.0 {
        return i16::MIN;
    }
    if sample >= 1.0 {
        return i16::MAX;
    }
    (sample * f32::from(i16::MAX)).round() as i16
}

fn backend_error(error: &WindowsError) -> OutputError {
    OutputError::Backend(error.to_string())
}

fn run_in_mta<T>(
    task: impl FnOnce() -> Result<T, OutputError> + Send + 'static,
) -> Result<T, OutputError>
where
    T: Send + 'static,
{
    thread::Builder::new()
        .name("sonimesh-wasapi-control".to_owned())
        .spawn(move || {
            let _apartment = ComApartment::initialize()?;
            task()
        })
        .map_err(|error| OutputError::Backend(error.to_string()))?
        .join()
        .map_err(|_| OutputError::Backend("direct WASAPI control worker panicked".to_owned()))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mix() -> MixFormat {
        MixFormat {
            sample_rate_hz: 48_000,
            channels: 2,
            sample_format: SampleFormat::Float32,
        }
    }

    #[test]
    fn requires_an_exact_mix_format() {
        let config = OutputConfig {
            sample_rate_hz: 48_000,
            channels: 2,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: Some(480),
            maximum_callback_frames: 960,
        };
        assert_eq!(validate_mix_config(mix(), config), Ok(()));

        let changed_rate = OutputConfig {
            sample_rate_hz: 44_100,
            ..config
        };
        assert_eq!(
            validate_mix_config(mix(), changed_rate),
            Err(OutputError::UnsupportedFormat)
        );
    }

    #[test]
    fn requested_duration_preserves_a_ten_millisecond_request() {
        let config = OutputConfig {
            sample_rate_hz: 48_000,
            channels: 2,
            sample_format: SampleFormat::Float32,
            requested_callback_frames: Some(480),
            maximum_callback_frames: 960,
        };
        assert_eq!(requested_duration(config), 100_000);
    }

    #[test]
    fn float32_sanitization_rejects_non_finite_and_bounds_samples() {
        assert_eq!(sanitize_float32(f32::NAN).to_bits(), 0.0_f32.to_bits());
        assert_eq!(
            sanitize_float32(f32::NEG_INFINITY).to_bits(),
            0.0_f32.to_bits()
        );
        assert_eq!(sanitize_float32(-2.0).to_bits(), (-1.0_f32).to_bits());
        assert_eq!(sanitize_float32(2.0).to_bits(), 1.0_f32.to_bits());
    }

    #[test]
    fn signed16_conversion_sanitizes_and_bounds_samples() {
        assert_eq!(float_to_i16(f32::NAN), 0);
        assert_eq!(float_to_i16(-2.0), i16::MIN);
        assert_eq!(float_to_i16(2.0), i16::MAX);
    }
}
