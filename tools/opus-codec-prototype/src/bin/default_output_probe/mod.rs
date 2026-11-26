use sonimesh_app::OutputSelection;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};

type ProbeResult = Result<OutputSelection, String>;

pub(crate) struct DefaultOutputProbe {
    requests: SyncSender<u64>,
    results: Receiver<(u64, ProbeResult)>,
    pending: Option<u64>,
    next_id: u64,
}

impl DefaultOutputProbe {
    pub(crate) fn new(
        mut probe: impl FnMut() -> ProbeResult + Send + 'static,
    ) -> std::io::Result<Self> {
        let (requests, incoming) = mpsc::sync_channel(1);
        let (outgoing, results) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("lab-default-output-probe".into())
            .spawn(move || {
                while let Ok(id) = incoming.recv() {
                    if outgoing.send((id, probe())).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            results,
            pending: None,
            next_id: 0,
        })
    }

    pub(crate) fn invalidate(&mut self) {
        self.pending = None;
    }

    pub(crate) fn poll(&mut self) -> ProbeResult {
        loop {
            match self.results.try_recv() {
                Ok((id, result)) if self.pending == Some(id) => {
                    self.pending = None;
                    return result;
                }
                Ok(_) => continue,
                Err(TryRecvError::Disconnected) => return Err("output probe stopped".into()),
                Err(TryRecvError::Empty) => break,
            }
        }
        if self.pending.is_none() {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            if self.requests.try_send(id).is_ok() {
                self.pending = Some(id);
            }
        }
        Err("output probe pending".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonimesh_platform::{OutputConfig, OutputDeviceId, SampleFormat};
    use std::time::Duration;

    #[test]
    fn slow_probe_is_nonblocking_and_invalidated_result_is_discarded() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let mut count = 0;
        let mut worker = DefaultOutputProbe::new(move || {
            count += 1;
            started_tx.send(count).unwrap();
            if count == 1 {
                release_rx.recv().unwrap();
            }
            finished_tx.send(count).unwrap();
            Ok(OutputSelection {
                device: Some(OutputDeviceId::new(format!("device-{count}"))),
                config: OutputConfig {
                    sample_rate_hz: 48_000,
                    channels: 2,
                    sample_format: SampleFormat::Float32,
                    requested_callback_frames: None,
                    maximum_callback_frames: 960,
                },
            })
        })
        .unwrap();
        assert!(worker.poll().is_err());
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        assert!(worker.poll().is_err());
        worker.invalidate();
        assert!(worker.poll().is_err());
        release_tx.send(()).unwrap();
        assert_eq!(finished_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
        assert_eq!(finished_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 2);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let selection = loop {
            if let Ok(selection) = worker.poll() {
                break selection;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert_eq!(selection.device, Some(OutputDeviceId::new("device-2")));
    }
}
