//! Windows system playback connection experiment, separate from the WLAN engine.

#[cfg(any(windows, test))]
use std::error::Error;
#[cfg(windows)]
use std::io::{self, Write};

#[cfg(any(windows, test))]
type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[cfg(any(windows, test))]
trait PlaybackConnection {
    fn start(&self) -> Result<()>;
    fn open(&self) -> Result<()>;
    fn close(&self) -> Result<()>;
}

#[cfg(any(windows, test))]
fn run_connection(
    connection: &impl PlaybackConnection,
    wait: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let result = connection
        .start()
        .and_then(|()| connection.open())
        .and_then(|()| wait());
    let close_result = connection.close();
    match (result, close_result) {
        (Err(error), Err(close_error)) => {
            eprintln!("Closing the playback connection also failed: {close_error}");
            Err(error)
        }
        (Err(error), _) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

#[cfg(any(windows, test))]
fn selection(input: &str, count: u32) -> Option<u32> {
    input
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|index| *index > 0 && *index <= count)
        .map(|index| index - 1)
}

#[cfg(windows)]
impl PlaybackConnection for windows::Media::Audio::AudioPlaybackConnection {
    fn start(&self) -> Result<()> {
        self.Start()?;
        Ok(())
    }

    fn open(&self) -> Result<()> {
        use windows::Media::Audio::AudioPlaybackConnectionOpenResultStatus;

        let result = self.Open()?;
        let status = result.Status()?;
        if status != AudioPlaybackConnectionOpenResultStatus::Success {
            return Err(format!(
                "System playback connection could not be opened (status {}, error {:?})",
                status.0,
                result.ExtendedError()?
            )
            .into());
        }
        Ok(())
    }

    fn close(&self) -> Result<()> {
        self.Close()?;
        Ok(())
    }
}

#[cfg(windows)]
fn main() -> Result<()> {
    use windows::Devices::Enumeration::DeviceInformation;
    use windows::Media::Audio::AudioPlaybackConnection;

    let mut arguments = std::env::args().skip(1);
    let connect = match (arguments.next().as_deref(), arguments.next()) {
        (None, None) => false,
        (Some("--connect"), None) => true,
        _ => return Err("Usage: windows-bluetooth-prototype [--connect]".into()),
    };
    let selector = AudioPlaybackConnection::GetDeviceSelector()?;
    let devices = DeviceInformation::FindAllAsyncAqsFilter(&selector)?.join()?;
    let count = devices.Size()?;
    println!("System playback connection candidates: {count}");
    let shown = count.min(64);
    for index in 0..shown {
        let device = devices.GetAt(index)?;
        println!("{}: {}", index + 1, device.Name()?);
    }
    if count > shown {
        println!("Only the first {shown} candidates are shown.");
    }
    if !connect || shown == 0 {
        println!(
            "No connection was opened. Discovery does not establish application PCM availability."
        );
        return Ok(());
    }

    print!("Choose a candidate number: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let index = selection(&input, shown).ok_or("Invalid candidate number")?;
    let device = devices.GetAt(index)?;
    let connection = AudioPlaybackConnection::TryCreateFromId(&device.Id()?)?;
    run_connection(&connection, || {
        println!("System playback connection opened. Press Enter to close it.");
        println!("An open connection does not confirm audible playback or application PCM.");
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        Ok(())
    })?;
    println!("System playback connection closed.");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("This experiment requires Windows.");
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeConnection {
        calls: RefCell<Vec<&'static str>>,
        fail: Option<&'static str>,
    }

    impl FakeConnection {
        fn call(&self, name: &'static str) -> Result<()> {
            self.calls.borrow_mut().push(name);
            if self.fail == Some(name) {
                Err(name.into())
            } else {
                Ok(())
            }
        }
    }

    impl PlaybackConnection for FakeConnection {
        fn start(&self) -> Result<()> {
            self.call("start")
        }
        fn open(&self) -> Result<()> {
            self.call("open")
        }
        fn close(&self) -> Result<()> {
            self.call("close")
        }
    }

    #[test]
    fn selection_requires_an_existing_one_based_index() {
        assert_eq!(selection(" 2\n", 2), Some(1));
        for input in ["0", "3", "-1", "", "x"] {
            assert_eq!(selection(input, 2), None);
        }
        assert_eq!(selection("1", 0), None);
    }

    #[test]
    fn connection_closes_after_success_or_failure() {
        for (failure, expected) in [
            (None, vec!["start", "open", "wait", "close"]),
            (Some("start"), vec!["start", "close"]),
            (Some("open"), vec!["start", "open", "close"]),
            (Some("wait"), vec!["start", "open", "wait", "close"]),
            (Some("close"), vec!["start", "open", "wait", "close"]),
        ] {
            let connection = FakeConnection {
                calls: RefCell::new(Vec::new()),
                fail: failure,
            };
            let result = run_connection(&connection, || connection.call("wait"));
            assert_eq!(result.is_err(), failure.is_some());
            assert_eq!(*connection.calls.borrow(), expected);
        }
    }
}
