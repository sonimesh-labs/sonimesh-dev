use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidates {
    generation: u32,
    names: Vec<String>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackState {
    Idle,
    Waiting,
    Open,
}

#[cfg(windows)]
mod platform {
    use super::{Candidates, PlaybackState};
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Media::Audio::{
        AudioPlaybackConnection, AudioPlaybackConnectionOpenResultStatus,
        AudioPlaybackConnectionState,
    };

    fn open_error(status: AudioPlaybackConnectionOpenResultStatus) -> &'static str {
        if status == AudioPlaybackConnectionOpenResultStatus::RequestTimedOut {
            "Windows timed out connecting. Check that the source is nearby and try again."
        } else if status == AudioPlaybackConnectionOpenResultStatus::DeniedBySystem {
            "Windows denied the playback connection. Check the device's Bluetooth connection and try again."
        } else {
            "Windows could not open the playback connection. Check the source and try again."
        }
    }

    #[derive(Default)]
    pub struct Bluetooth {
        devices: Vec<DeviceInformation>,
        generation: u32,
        connection: Option<AudioPlaybackConnection>,
        opened: bool,
    }

    impl Bluetooth {
        pub fn refresh(&mut self) -> Result<Candidates, String> {
            let selector =
                AudioPlaybackConnection::GetDeviceSelector().map_err(|e| e.to_string())?;
            let collection = DeviceInformation::FindAllAsyncAqsFilter(&selector)
                .map_err(|e| e.to_string())?
                .join()
                .map_err(|e| e.to_string())?;
            let count = collection.Size().map_err(|e| e.to_string())?.min(64);
            let mut devices = Vec::new();
            let mut names = Vec::new();
            for index in 0..count {
                let device = collection.GetAt(index).map_err(|e| e.to_string())?;
                names.push(device.Name().map_err(|e| e.to_string())?.to_string_lossy());
                devices.push(device);
            }
            self.devices = devices;
            self.generation = self.generation.wrapping_add(1);
            Ok(Candidates {
                generation: self.generation,
                names,
            })
        }

        fn start(
            &mut self,
            generation: u32,
            index: usize,
        ) -> Result<AudioPlaybackConnection, String> {
            if self.connection.is_some() {
                return Err("Disconnect the current source first.".into());
            }
            if generation != self.generation {
                return Err("The device list changed. Select a source again.".into());
            }
            let device = self.devices.get(index).ok_or("Select a listed source.")?;
            let connection =
                AudioPlaybackConnection::TryCreateFromId(&device.Id().map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
            if let Err(error) = connection.Start() {
                let _ = connection.Close();
                return Err(error.to_string());
            }
            Ok(connection)
        }

        pub fn prepare(&mut self, generation: u32, index: usize) -> Result<(), String> {
            let connection = self.start(generation, index)?;
            self.connection = Some(connection);
            self.opened = false;
            Ok(())
        }

        pub fn connect(&mut self, generation: u32, index: usize) -> Result<(), String> {
            let connection = self.start(generation, index)?;
            let result: Result<(), String> = (|| {
                let result = connection.Open().map_err(|e| e.to_string())?;
                let status = result.Status().map_err(|e| e.to_string())?;
                if status != AudioPlaybackConnectionOpenResultStatus::Success {
                    return Err(open_error(status).into());
                }
                Ok(())
            })();
            if result.is_err() {
                let _ = connection.Close();
            }
            result?;
            self.connection = Some(connection);
            self.opened = true;
            Ok(())
        }

        pub fn status(&mut self) -> Result<PlaybackState, String> {
            let Some(connection) = &self.connection else {
                return Ok(PlaybackState::Idle);
            };
            let state = connection.State().map_err(|e| e.to_string())?;
            if state == AudioPlaybackConnectionState::Opened {
                self.opened = true;
                return Ok(PlaybackState::Open);
            }
            if state != AudioPlaybackConnectionState::Closed {
                return Err("Unknown playback connection state.".into());
            }
            if !self.opened {
                return Ok(PlaybackState::Waiting);
            }
            self.disconnect()?;
            Ok(PlaybackState::Idle)
        }

        pub fn disconnect(&mut self) -> Result<(), String> {
            self.opened = false;
            if let Some(connection) = self.connection.take() {
                connection.Close().map_err(|e| e.to_string())?;
            }
            Ok(())
        }
    }

    impl Drop for Bluetooth {
        fn drop(&mut self) {
            let _ = self.disconnect();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{Bluetooth, PlaybackState, open_error};
        use windows::Media::Audio::AudioPlaybackConnectionOpenResultStatus;

        #[test]
        fn open_failures_explain_what_happened() {
            assert!(
                open_error(AudioPlaybackConnectionOpenResultStatus::RequestTimedOut)
                    .contains("timed out")
            );
            assert!(
                open_error(AudioPlaybackConnectionOpenResultStatus::DeniedBySystem)
                    .contains("denied")
            );
            assert!(
                open_error(AudioPlaybackConnectionOpenResultStatus::UnknownFailure)
                    .contains("could not open")
            );
        }

        #[test]
        fn connection_requires_a_current_list_and_valid_selection() {
            let mut bluetooth = Bluetooth::default();
            assert!(bluetooth.connect(1, 0).is_err());
            assert!(bluetooth.connect(0, 0).is_err());
            assert_eq!(bluetooth.status().unwrap(), PlaybackState::Idle);
            bluetooth.disconnect().unwrap();
        }

        #[test]
        #[ignore = "queries the Windows playback device inventory"]
        fn windows_discovery_returns_a_usable_snapshot() {
            let mut bluetooth = Bluetooth::default();
            let snapshot = bluetooth.refresh().unwrap();
            assert_eq!(snapshot.generation, 1);
            assert!(snapshot.names.len() <= 64);
            assert!(bluetooth.connect(0, 0).is_err());
        }

        #[test]
        #[ignore = "opens the sole Windows playback candidate; requires a test source"]
        fn real_connection_opens_reports_state_and_closes() {
            let mut bluetooth = Bluetooth::default();
            let snapshot = bluetooth.refresh().unwrap();
            assert_eq!(
                snapshot.names.len(),
                1,
                "the test requires exactly one candidate"
            );
            bluetooth
                .connect(snapshot.generation, 0)
                .expect("opening the system playback connection failed");
            assert_eq!(
                bluetooth.status().expect("could not read connection state"),
                PlaybackState::Open
            );
            bluetooth
                .disconnect()
                .expect("closing the system playback connection failed");
            assert_eq!(bluetooth.status().unwrap(), PlaybackState::Idle);
        }

        #[test]
        #[ignore = "starts a short Windows playback wait; requires exactly one candidate"]
        fn real_windows_inbound_wait_reports_state_and_closes() {
            let mut bluetooth = Bluetooth::default();
            let snapshot = bluetooth.refresh().unwrap();
            assert_eq!(
                snapshot.names.len(),
                1,
                "the test requires exactly one candidate"
            );
            bluetooth
                .prepare(snapshot.generation, 0)
                .expect("could not prepare Windows playback");
            let state = bluetooth
                .status()
                .expect("could not read Windows playback state");
            assert!(matches!(
                state,
                PlaybackState::Waiting | PlaybackState::Open
            ));
            std::thread::sleep(std::time::Duration::from_secs(2));
            let _ = bluetooth
                .status()
                .expect("could not poll Windows playback state");
            bluetooth
                .disconnect()
                .expect("could not close Windows playback wait");
            assert_eq!(bluetooth.status().unwrap(), PlaybackState::Idle);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::{Candidates, PlaybackState};
    #[derive(Default)]
    pub struct Bluetooth;
    impl Bluetooth {
        pub fn refresh(&mut self) -> Result<Candidates, String> {
            Err("Bluetooth playback experiment requires Windows.".into())
        }
        pub fn connect(&mut self, _: u32, _: usize) -> Result<(), String> {
            Err("Bluetooth playback experiment requires Windows.".into())
        }
        pub fn prepare(&mut self, _: u32, _: usize) -> Result<(), String> {
            Err("Bluetooth playback experiment requires Windows.".into())
        }
        pub fn status(&mut self) -> Result<PlaybackState, String> {
            Ok(PlaybackState::Idle)
        }
        pub fn disconnect(&mut self) -> Result<(), String> {
            Ok(())
        }
    }
}

pub use platform::Bluetooth;
