use sonimesh_app::OutputSelection;
use sonimesh_platform::OutputDeviceId;
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};

type ProbeResult = Result<OutputSelection, String>;

pub(crate) struct DefaultOutputProbe {
    requests: SyncSender<(u64, Option<OutputDeviceId>)>,
    results: Receiver<(u64, ProbeResult)>,
    pending: Option<u64>,
    next_id: u64,
    target: Option<OutputDeviceId>,
}

impl DefaultOutputProbe {
    pub(crate) fn new(
        target: Option<OutputDeviceId>,
        mut probe: impl FnMut(Option<&OutputDeviceId>) -> ProbeResult + Send + 'static,
    ) -> std::io::Result<Self> {
        let (requests, incoming) = mpsc::sync_channel::<(u64, Option<OutputDeviceId>)>(1);
        let (outgoing, results) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("lab-default-output-probe".into())
            .spawn(move || {
                while let Ok((id, target)) = incoming.recv() {
                    if outgoing.send((id, probe(target.as_ref()))).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            results,
            pending: None,
            next_id: 0,
            target,
        })
    }

    pub(crate) fn invalidate(&mut self) {
        self.pending = None;
    }

    pub(crate) fn set_target(&mut self, target: Option<&OutputDeviceId>) {
        if self.target.as_ref() != target {
            self.target = target.cloned();
            self.invalidate();
        }
    }

    pub(crate) fn poll_result(&mut self) -> Option<ProbeResult> {
        loop {
            match self.results.try_recv() {
                Ok((id, result)) if self.pending == Some(id) => {
                    self.pending = None;
                    return Some(result);
                }
                Ok(_) => continue,
                Err(TryRecvError::Disconnected) => return Some(Err("output probe stopped".into())),
                Err(TryRecvError::Empty) => break,
            }
        }
        if self.pending.is_none() {
            let id = self.next_id;
            self.next_id = self.next_id.wrapping_add(1);
            if self.requests.try_send((id, self.target.clone())).is_ok() {
                self.pending = Some(id);
            }
        }
        None
    }

    pub(crate) fn poll(&mut self) -> ProbeResult {
        self.poll_result()
            .unwrap_or_else(|| Err("output probe pending".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sonimesh_platform::{OutputConfig, SampleFormat};
    use std::time::Duration;

    #[test]
    fn slow_probe_is_nonblocking_and_invalidated_result_is_discarded() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let mut count = 0;
        let mut worker = DefaultOutputProbe::new(None, move |target| {
            count += 1;
            started_tx.send((count, target.cloned())).unwrap();
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
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            (1, None)
        );
        assert!(worker.poll().is_err());
        worker.set_target(Some(&OutputDeviceId::new("new-target")));
        assert!(worker.poll().is_err());
        release_tx.send(()).unwrap();
        assert_eq!(finished_rx.recv_timeout(Duration::from_secs(2)).unwrap(), 1);
        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            (2, Some(OutputDeviceId::new("new-target")))
        );
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

    #[test]
    fn pending_probe_is_distinct_from_query_failure() {
        let (release_tx, release_rx) = mpsc::channel();
        let mut worker = DefaultOutputProbe::new(None, move |_| {
            release_rx.recv().unwrap();
            Err("device unavailable".into())
        })
        .unwrap();
        assert!(worker.poll_result().is_none());
        assert!(worker.poll_result().is_none());
        release_tx.send(()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(result) = worker.poll_result() {
                assert_eq!(result.unwrap_err(), "device unavailable");
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
