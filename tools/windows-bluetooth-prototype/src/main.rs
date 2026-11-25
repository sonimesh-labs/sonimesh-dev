//! Read-only discovery experiment for Windows system playback connections.

#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Media::Audio::AudioPlaybackConnection;

    let selector = AudioPlaybackConnection::GetDeviceSelector()?;
    let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)?.join()?;
    println!("System playback connection candidates: {}", devices.Size()?);
    println!("Discovery only: no connection or application PCM was tested.");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This discovery experiment requires Windows.");
    std::process::exit(1);
}
