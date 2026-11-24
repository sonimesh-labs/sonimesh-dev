//! Controlled-lab UDP relay; live socket timing is not deterministic.

use std::error::Error;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::thread;
use std::time::{Duration, Instant};

use sonimesh_network_simulator::{NetworkProfile, NetworkStats, VirtualNetwork};
use sonimesh_protocol::MAX_MEDIA_DATAGRAM_LEN;

const RECEIVE_BATCH: usize = 64;

#[derive(Debug)]
struct Options {
    listen: SocketAddrV4,
    forward: SocketAddrV4,
    duration: Duration,
    report_interval: Option<Duration>,
    profile: NetworkProfile,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, Box<dyn Error>> {
        let mut result = Self {
            listen: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 39093),
            forward: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 39094),
            duration: Duration::from_secs(60),
            report_interval: None,
            profile: NetworkProfile::stable(1),
        };
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let value = args.next().ok_or("option requires a value")?;
            match flag.as_str() {
                "--listen" => result.listen = value.parse()?,
                "--forward" => result.forward = value.parse()?,
                "--seconds" => result.duration = Duration::from_secs(value.parse()?),
                "--report-seconds" => {
                    result.report_interval = Some(Duration::from_secs(value.parse()?));
                }
                "--seed" => result.profile.seed = value.parse()?,
                "--loss" => result.profile.loss_percent = value.parse()?,
                "--duplicate" => result.profile.duplicate_percent = value.parse()?,
                "--reorder" => result.profile.reorder_percent = value.parse()?,
                "--burst-interval" => result.profile.burst_interval_packets = value.parse()?,
                "--burst-length" => result.profile.burst_length_packets = value.parse()?,
                "--delay-ms" => result.profile.base_delay_ms = value.parse()?,
                "--jitter-ms" => result.profile.jitter_ms = value.parse()?,
                "--reorder-delay-ms" => result.profile.reorder_delay_ms = value.parse()?,
                "--bandwidth-bytes-per-second" => {
                    result.profile.bandwidth_bytes_per_second = value.parse()?;
                }
                "--queue-packets" => result.profile.max_in_flight_packets = value.parse()?,
                _ => return Err("unknown option".into()),
            }
        }
        if result.listen.port() == 0
            || result.forward.port() == 0
            || result.listen.port() == result.forward.port()
            || result.forward.ip().is_unspecified()
        {
            return Err(
                "listen and forward must use distinct nonzero ports and a concrete forward address"
                    .into(),
            );
        }
        if !(1..=86_400).contains(&result.duration.as_secs()) {
            return Err("seconds must be between 1 and 86400".into());
        }
        if result
            .report_interval
            .is_some_and(|interval| !(1..=3_600).contains(&interval.as_secs()))
        {
            return Err("report-seconds must be between 1 and 3600".into());
        }
        result.profile.validate()?;
        Ok(result)
    }
}

#[derive(Debug)]
struct RelayReport {
    network: NetworkStats,
    oversized: u64,
    forwarded: u64,
    queued: usize,
    received_bytes: u64,
    forwarded_bytes: u64,
    peak_queued: usize,
}

fn relay(
    options: &Options,
    input: &UdpSocket,
    output: &UdpSocket,
    mut on_report: impl FnMut(Duration, &RelayReport),
) -> Result<RelayReport, Box<dyn Error>> {
    input.set_nonblocking(true)?;
    let mut network = VirtualNetwork::new(options.profile)?;
    let mut buffer = vec![0_u8; 65_536];
    let start = Instant::now();
    let mut oversized = 0_u64;
    let mut forwarded = 0_u64;
    let mut received_bytes = 0_u64;
    let mut forwarded_bytes = 0_u64;
    let mut peak_queued = 0_usize;
    let mut next_report = options.report_interval;
    while start.elapsed() < options.duration {
        for _ in 0..RECEIVE_BATCH {
            match input.recv_from(&mut buffer) {
                Ok((size, _)) if size <= MAX_MEDIA_DATAGRAM_LEN => {
                    received_bytes = received_bytes.saturating_add(size as u64);
                    let now = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
                    network.send(now, buffer[..size].to_vec(), size as u64);
                    peak_queued = peak_queued.max(network.queued_copies());
                }
                Ok(_) => oversized = oversized.saturating_add(1),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error.into()),
            }
        }
        let now = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        for delivery in network.drain_ready(now) {
            output.send_to(&delivery.payload, options.forward)?;
            forwarded = forwarded.saturating_add(1);
            forwarded_bytes = forwarded_bytes.saturating_add(delivery.payload.len() as u64);
        }
        let elapsed = start.elapsed();
        if next_report.is_some_and(|due| elapsed >= due) {
            on_report(
                elapsed,
                &RelayReport {
                    network: network.stats(),
                    oversized,
                    forwarded,
                    queued: network.queued_copies(),
                    received_bytes,
                    forwarded_bytes,
                    peak_queued,
                },
            );
            next_report = options
                .report_interval
                .map(|interval| start.elapsed() + interval);
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(RelayReport {
        network: network.stats(),
        oversized,
        forwarded,
        queued: network.queued_copies(),
        received_bytes,
        forwarded_bytes,
        peak_queued,
    })
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = Options::parse(std::env::args().skip(1)).map_err(|error| {
        format!("{error}; usage: udp-impairment-relay [--listen IPv4:PORT] [--forward IPv4:PORT] [--seconds 1-86400] [--report-seconds 1-3600] [--seed N] [--loss 0-100] [--duplicate 0-100] [--reorder 0-100] [--burst-interval N] [--burst-length N] [--delay-ms N] [--jitter-ms N] [--reorder-delay-ms N] [--bandwidth-bytes-per-second 0-1000000000] [--queue-packets 1-4096]")
    })?;
    let input = UdpSocket::bind(options.listen)?;
    let output = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
    println!(
        "relay listen={} forward={} seconds={} profile={:?}",
        options.listen,
        options.forward,
        options.duration.as_secs(),
        options.profile
    );
    let report = relay(&options, &input, &output, |elapsed, report| {
        println!(
            "relay elapsed_ms={} forwarded={} oversized={} queued={} received_bytes={} forwarded_bytes={} peak_queued={} stats={:?}",
            elapsed.as_millis(),
            report.forwarded,
            report.oversized,
            report.queued,
            report.received_bytes,
            report.forwarded_bytes,
            report.peak_queued,
            report.network
        );
    })?;
    println!(
        "relay forwarded={} oversized={} queued_at_stop={} received_bytes={} forwarded_bytes={} peak_queued={} stats={:?}",
        report.forwarded,
        report.oversized,
        report.queued,
        report.received_bytes,
        report.forwarded_bytes,
        report.peak_queued,
        report.network
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn validates_ports_profiles_and_duration() {
        assert!(Options::parse(args(&[])).is_ok());
        let defaults = Options::parse(args(&[])).unwrap();
        assert_eq!(defaults.profile.bandwidth_bytes_per_second, 0);
        assert_eq!(defaults.profile.max_in_flight_packets, 256);
        let shaped = Options::parse(args(&[
            "--bandwidth-bytes-per-second",
            "1000000000",
            "--queue-packets",
            "4096",
        ]))
        .unwrap();
        assert_eq!(shaped.profile.bandwidth_bytes_per_second, 1_000_000_000);
        assert_eq!(shaped.profile.max_in_flight_packets, 4096);
        for seconds in ["1", "28800", "86400"] {
            assert_eq!(
                Options::parse(args(&["--seconds", seconds]))
                    .unwrap()
                    .duration,
                Duration::from_secs(seconds.parse().unwrap())
            );
        }
        assert_eq!(
            Options::parse(args(&["--report-seconds", "5"]))
                .unwrap()
                .report_interval,
            Some(Duration::from_secs(5))
        );
        for values in [
            vec!["--listen", "127.0.0.1:39094"],
            vec!["--forward", "0.0.0.0:1"],
            vec!["--seconds", "0"],
            vec!["--seconds", "86401"],
            vec!["--report-seconds", "0"],
            vec!["--report-seconds", "3601"],
            vec!["--loss", "101"],
            vec!["--delay-ms", "60001"],
            vec!["--burst-length", "1"],
            vec!["--burst-interval", "2", "--burst-length", "3"],
            vec!["--bandwidth-bytes-per-second", "1000000001"],
            vec!["--queue-packets", "0"],
            vec!["--queue-packets", "4097"],
            vec!["--seed"],
        ] {
            assert!(Options::parse(args(&values)).is_err());
        }
    }

    #[test]
    fn shapes_live_datagrams_and_drops_when_the_queue_fills() {
        let input = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
        let output = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut options = Options::parse(args(&[
            "--bandwidth-bytes-per-second",
            "100",
            "--queue-packets",
            "2",
            "--delay-ms",
            "0",
            "--jitter-ms",
            "0",
        ]))
        .unwrap();
        options.forward = match sink.local_addr().unwrap() {
            std::net::SocketAddr::V4(address) => address,
            std::net::SocketAddr::V6(_) => unreachable!(),
        };
        options.duration = Duration::from_millis(2_300);
        options.report_interval = Some(Duration::from_millis(50));
        for _ in 0..3 {
            sender
                .send_to(&[1_u8; 100], input.local_addr().unwrap())
                .unwrap();
        }
        let report = relay(&options, &input, &output, |elapsed, report| {
            if elapsed < Duration::from_secs(1) {
                assert_eq!(report.forwarded, 0);
            } else if elapsed < Duration::from_secs(2) {
                assert!(report.forwarded <= 1);
            }
        })
        .unwrap();
        assert_eq!(report.network.submitted_packets, 3);
        assert_eq!(report.network.bandwidth_shaped_copies, 2);
        assert_eq!(report.network.bandwidth_serialized_bytes, 200);
        assert_eq!(report.network.capacity_dropped_copies, 1);
        assert_eq!(report.forwarded, 2);
        assert_eq!(report.queued, 0);
        assert_eq!(report.received_bytes, 300);
        assert_eq!(report.forwarded_bytes, 200);
        assert_eq!(report.peak_queued, 2);
        sink.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut received = [0_u8; 100];
        for _ in 0..2 {
            let (length, _) = sink.recv_from(&mut received).unwrap();
            assert_eq!(length, 100);
            assert_eq!(received, [1; 100]);
        }
    }

    #[test]
    fn reports_cumulative_progress_from_live_udp() {
        let input = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
        let output = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut options = Options::parse(args(&[])).unwrap();
        options.forward = match sink.local_addr().unwrap() {
            std::net::SocketAddr::V4(address) => address,
            std::net::SocketAddr::V6(_) => unreachable!(),
        };
        options.duration = Duration::from_millis(250);
        options.report_interval = Some(Duration::from_millis(50));
        options.profile.base_delay_ms = 0;
        options.profile.jitter_ms = 0;
        sender
            .send_to(b"media", input.local_addr().unwrap())
            .unwrap();
        sender
            .send_to(
                &vec![0; MAX_MEDIA_DATAGRAM_LEN + 1],
                input.local_addr().unwrap(),
            )
            .unwrap();
        let mut reports = Vec::new();
        let final_report = relay(&options, &input, &output, |elapsed, report| {
            reports.push((
                elapsed,
                report.network.submitted_packets,
                report.forwarded,
                report.oversized,
                report.queued,
                report.received_bytes,
                report.forwarded_bytes,
                report.peak_queued,
            ));
        })
        .unwrap();
        assert!(!reports.is_empty());
        assert!(reports.windows(2).all(|pair| pair[0].0 < pair[1].0));
        assert!(reports.iter().all(
            |&(
                _,
                submitted,
                forwarded,
                oversized,
                queued,
                received_bytes,
                forwarded_bytes,
                peak_queued,
            )| (
                submitted,
                forwarded,
                oversized,
                queued,
                received_bytes,
                forwarded_bytes,
                peak_queued
            ) == (1, 1, 1, 0, 5, 5, 1)
        ));
        assert_eq!(final_report.network.submitted_packets, 1);
        assert_eq!(final_report.forwarded, 1);
        assert_eq!(final_report.oversized, 1);
        assert_eq!(final_report.received_bytes, 5);
        assert_eq!(final_report.forwarded_bytes, 5);
        assert_eq!(final_report.peak_queued, 1);
        sink.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut bytes = [0; 16];
        let (size, _) = sink.recv_from(&mut bytes).unwrap();
        assert_eq!(&bytes[..size], b"media");
    }

    #[test]
    fn forwards_only_packets_outside_configured_loss_bursts() {
        let input = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
        sink.set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let output = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut options = Options::parse(args(&[
            "--burst-interval",
            "3",
            "--burst-length",
            "2",
            "--delay-ms",
            "0",
            "--jitter-ms",
            "0",
        ]))
        .unwrap();
        options.forward = match sink.local_addr().unwrap() {
            std::net::SocketAddr::V4(address) => address,
            std::net::SocketAddr::V6(_) => unreachable!(),
        };
        options.duration = Duration::from_millis(250);
        for sequence in 0..9_u8 {
            sender
                .send_to(&[sequence], input.local_addr().unwrap())
                .unwrap();
        }
        let report = relay(&options, &input, &output, |_, _| {}).unwrap();
        let mut received = Vec::new();
        let mut bytes = [0_u8; 1];
        while sink.recv_from(&mut bytes).is_ok() {
            received.push(bytes[0]);
        }
        assert_eq!(received, [0, 1, 2, 5, 8]);
        assert_eq!(report.network.submitted_packets, 9);
        assert_eq!(report.network.burst_loss_packets, 4);
        assert_eq!(report.forwarded, 5);
    }

    #[test]
    fn forwards_datagrams_and_applies_loss_on_real_loopback_sockets() {
        for loss in [0, 100] {
            let input = UdpSocket::bind("127.0.0.1:0").unwrap();
            let input_address = input.local_addr().unwrap();
            let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
            receiver
                .set_read_timeout(Some(Duration::from_millis(500)))
                .unwrap();
            let output = UdpSocket::bind("127.0.0.1:0").unwrap();
            let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
            let mut options = Options::parse(args(&[])).unwrap();
            options.forward = match receiver.local_addr().unwrap() {
                std::net::SocketAddr::V4(address) => address,
                std::net::SocketAddr::V6(_) => unreachable!(),
            };
            options.duration = Duration::from_millis(250);
            options.profile.loss_percent = loss;
            options.profile.base_delay_ms = 10;
            options.profile.jitter_ms = 0;
            sender.send_to(b"media", input_address).unwrap();
            let handle =
                thread::spawn(move || relay(&options, &input, &output, |_, _| {}).unwrap());
            let mut bytes = [0; 16];
            let read_result = receiver.recv_from(&mut bytes);
            let report = handle.join().unwrap();
            assert_eq!(report.network.submitted_packets, 1);
            if loss == 0 {
                assert_eq!(&bytes[..read_result.unwrap().0], b"media");
                assert_eq!(report.forwarded, 1);
            } else {
                assert!(read_result.is_err());
                assert_eq!(report.network.random_loss_packets, 1);
                assert_eq!(report.forwarded, 0);
            }
        }
    }

    #[test]
    fn duplicates_valid_datagrams_and_discards_oversized_input() {
        let input = UdpSocket::bind("127.0.0.1:0").unwrap();
        let input_address = input.local_addr().unwrap();
        let sink = UdpSocket::bind("127.0.0.1:0").unwrap();
        sink.set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let output = UdpSocket::bind("127.0.0.1:0").unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut options = Options::parse(args(&[])).unwrap();
        options.forward = match sink.local_addr().unwrap() {
            std::net::SocketAddr::V4(address) => address,
            std::net::SocketAddr::V6(_) => unreachable!(),
        };
        options.duration = Duration::from_millis(250);
        options.profile.base_delay_ms = 0;
        options.profile.jitter_ms = 0;
        options.profile.duplicate_percent = 100;
        sender.send_to(b"media", input_address).unwrap();
        sender
            .send_to(&vec![0; MAX_MEDIA_DATAGRAM_LEN + 1], input_address)
            .unwrap();
        let handle = thread::spawn(move || relay(&options, &input, &output, |_, _| {}).unwrap());
        let mut bytes = [0; 16];
        for _ in 0..2 {
            let (size, _) = sink.recv_from(&mut bytes).unwrap();
            assert_eq!(&bytes[..size], b"media");
        }
        let report = handle.join().unwrap();
        assert_eq!(report.network.submitted_packets, 1);
        assert_eq!(report.network.duplicate_copies, 1);
        assert_eq!(report.oversized, 1);
        assert_eq!(report.forwarded, 2);
        assert_eq!(report.received_bytes, 5);
        assert_eq!(report.forwarded_bytes, 10);
        assert_eq!(report.peak_queued, 2);
    }
}
