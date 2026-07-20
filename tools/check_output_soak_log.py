#!/usr/bin/env python3
"""Validate retained Windows output-soak control-domain transcripts."""

from __future__ import annotations

import argparse
import math
import re
import sys
import unittest
from dataclasses import dataclass
from pathlib import Path


MAXIMUM_SOAK_SECONDS = 28_800
HEADER_PATTERN = re.compile(r"^(?P<name>soak duration|report interval) \(s\): (?P<value>\d+)$")
MAXIMUM_FRAMES_PATTERN = re.compile(r"^maximum frames: (?P<value>\d+)$")
REQUEST_PATTERN = re.compile(r"^requested frames: (?P<value>default|\d+)$")
FORMAT_PREFIX = "default format: "


@dataclass(frozen=True)
class BackendSpec:
    name: str
    count_key: str
    status_keys: tuple[str, ...]


BACKENDS = {
    "cpal": BackendSpec(
        name="CPAL",
        count_key="callbacks",
        status_keys=(
            "elapsed_s",
            "callbacks",
            "latest_frames",
            "max_frames",
            "latest_offset_ns",
            "max_gap_ns",
            "oversized",
            "errors",
            "health",
        ),
    ),
    "wasapi": BackendSpec(
        name="direct-WASAPI",
        count_key="renders",
        status_keys=(
            "elapsed_s",
            "renders",
            "latest_frames",
            "max_frames",
            "latest_offset_ns",
            "max_gap_ns",
            "endpoint_buffer_frames",
            "primed_frames",
            "oversized",
            "errors",
            "health",
        ),
    ),
}


@dataclass(frozen=True)
class SoakStatus:
    elapsed_seconds: int
    count: int
    latest_frames: int
    maximum_frames: int
    offset_nanoseconds: int
    maximum_gap_nanoseconds: int
    oversized: int
    errors: int
    health: str
    endpoint_buffer_frames: int | None = None
    primed_frames: int | None = None


@dataclass(frozen=True)
class SoakReport:
    duration_seconds: int
    report_interval_seconds: int
    default_format: str
    requested_frames: str
    maximum_frames: int
    periodic_statuses: tuple[SoakStatus, ...]
    final_status: SoakStatus


def parse_header(line: str) -> tuple[str, int] | None:
    match = HEADER_PATTERN.fullmatch(line)
    if match is None:
        return None
    return match.group("name"), int(match.group("value"))


def parse_configuration_header(line: str) -> tuple[str, str | int] | None:
    """Parses retained non-timing startup configuration without interpreting format."""
    if line.startswith(FORMAT_PREFIX):
        value = line.removeprefix(FORMAT_PREFIX)
        if not value:
            raise ValueError("default format header is empty")
        return "default format", value
    request = REQUEST_PATTERN.fullmatch(line)
    if request is not None:
        return "requested frames", request.group("value")
    maximum = MAXIMUM_FRAMES_PATTERN.fullmatch(line)
    if maximum is not None:
        return "maximum frames", int(maximum.group("value"))
    return None


def parse_status(line: str, spec: BackendSpec) -> tuple[bool, SoakStatus] | None:
    final = line.startswith("final ")
    text = line.removeprefix("final ")
    if not text.startswith("elapsed_s="):
        return None

    values: dict[str, str] = {}
    for field in text.split(", "):
        key, separator, value = field.partition("=")
        if not separator or not key or not value or key in values:
            raise ValueError(f"malformed {spec.name} status field: {field!r}")
        values[key] = value
    if tuple(values) != spec.status_keys:
        raise ValueError(
            f"unexpected {spec.name} status layout: expected {', '.join(spec.status_keys)}"
        )

    try:
        status = SoakStatus(
            elapsed_seconds=int(values["elapsed_s"]),
            count=int(values[spec.count_key]),
            latest_frames=int(values["latest_frames"]),
            maximum_frames=int(values["max_frames"]),
            offset_nanoseconds=int(values["latest_offset_ns"]),
            maximum_gap_nanoseconds=int(values["max_gap_ns"]),
            oversized=int(values["oversized"]),
            errors=int(values["errors"]),
            health=values["health"],
            endpoint_buffer_frames=(
                int(values["endpoint_buffer_frames"])
                if "endpoint_buffer_frames" in values
                else None
            ),
            primed_frames=int(values["primed_frames"]) if "primed_frames" in values else None,
        )
    except ValueError as error:
        raise ValueError(f"invalid {spec.name} numeric status field") from error
    return final, status


def validate_report(lines: list[str], backend: str) -> SoakReport:
    spec = BACKENDS[backend]
    headers: dict[str, str | int] = {}
    periodic_statuses: list[SoakStatus] = []
    final_status: SoakStatus | None = None

    for line in lines:
        header = parse_header(line)
        if header is not None:
            name, value = header
            if name in headers:
                raise ValueError(f"duplicate {name} header")
            headers[name] = value
            continue
        configuration = parse_configuration_header(line)
        if configuration is not None:
            name, value = configuration
            if name in headers:
                raise ValueError(f"duplicate {name} header")
            headers[name] = value
            continue
        parsed = parse_status(line, spec)
        if parsed is None:
            continue
        is_final, status = parsed
        if is_final:
            if final_status is not None:
                raise ValueError("multiple final statuses")
            final_status = status
        elif final_status is not None:
            raise ValueError("periodic status appears after final status")
        else:
            periodic_statuses.append(status)

    duration = headers.get("soak duration")
    interval = headers.get("report interval")
    default_format = headers.get("default format")
    requested_frames = headers.get("requested frames")
    maximum_frames = headers.get("maximum frames")
    if not isinstance(duration, int) or not isinstance(interval, int):
        raise ValueError("missing soak duration or report interval header")
    if not isinstance(default_format, str) or not isinstance(requested_frames, str):
        raise ValueError("missing default format or requested frames header")
    if not isinstance(maximum_frames, int):
        raise ValueError("missing maximum frames header")
    if not 1 <= duration <= MAXIMUM_SOAK_SECONDS:
        raise ValueError("soak duration is outside the accepted command bound")
    if not 1 <= interval <= duration:
        raise ValueError("report interval is outside the accepted command bound")
    if maximum_frames <= 0:
        raise ValueError("maximum frames header must be positive")
    if requested_frames != "default" and int(requested_frames) <= 0:
        raise ValueError("requested frames header must be default or positive")
    if requested_frames != "default" and int(requested_frames) > maximum_frames:
        raise ValueError("requested frames header exceeds declared maximum frames")
    if final_status is None:
        raise ValueError("missing final status")

    expected_periodic_statuses = math.ceil(duration / interval)
    if len(periodic_statuses) != expected_periodic_statuses:
        raise ValueError(
            "missing or extra periodic statuses: "
            f"expected {expected_periodic_statuses}, got {len(periodic_statuses)}"
        )

    previous: SoakStatus | None = None
    for index, status in enumerate(periodic_statuses, start=1):
        expected_elapsed = min(index * interval, duration)
        if status.elapsed_seconds < expected_elapsed:
            raise ValueError(
                "periodic status arrived before its configured report interval: "
                f"expected at least {expected_elapsed} seconds, got {status.elapsed_seconds}"
            )

    for status in periodic_statuses:
        validate_status(status, spec, previous, duration, maximum_frames=maximum_frames)
        previous = status
    validate_status(
        final_status,
        spec,
        previous,
        duration,
        final_status=True,
        maximum_frames=maximum_frames,
    )
    if final_status.elapsed_seconds < duration:
        raise ValueError("final status completed before the configured soak duration")

    return SoakReport(
        duration_seconds=duration,
        report_interval_seconds=interval,
        default_format=default_format,
        requested_frames=requested_frames,
        maximum_frames=maximum_frames,
        periodic_statuses=tuple(periodic_statuses),
        final_status=final_status,
    )


def validate_status(
    status: SoakStatus,
    spec: BackendSpec,
    previous: SoakStatus | None,
    duration: int,
    final_status: bool = False,
    maximum_frames: int = 0,
) -> None:
    if status.elapsed_seconds <= 0:
        raise ValueError("status elapsed time must be positive")
    if status.latest_frames <= 0 or status.maximum_frames < status.latest_frames:
        raise ValueError("status frame observations are invalid")
    if status.maximum_frames > maximum_frames:
        raise ValueError("observed frame quantum exceeds declared maximum frames")
    if status.count <= 0 or status.offset_nanoseconds <= 0:
        raise ValueError("status callback/render count or offset is invalid")
    if status.maximum_gap_nanoseconds < 0:
        raise ValueError("status maximum gap is invalid")
    if spec.name == "direct-WASAPI":
        if status.endpoint_buffer_frames is None or status.primed_frames is None:
            raise ValueError("direct-WASAPI status is missing endpoint or priming frames")
        if status.endpoint_buffer_frames <= 0 or status.primed_frames <= 0:
            raise ValueError("direct-WASAPI endpoint or priming frames are invalid")
        if status.endpoint_buffer_frames != status.primed_frames:
            raise ValueError("direct-WASAPI priming frames do not match endpoint buffer frames")
    if status.oversized != 0 or status.errors != 0 or status.health != "Healthy":
        raise ValueError(
            f"{spec.name} soak is unhealthy: oversized={status.oversized}, "
            f"errors={status.errors}, health={status.health}"
        )
    if previous is None:
        return
    if status.elapsed_seconds < previous.elapsed_seconds:
        raise ValueError("status elapsed time is not monotonic")
    if previous.elapsed_seconds == duration and status.elapsed_seconds != duration:
        raise ValueError("status exceeds configured duration before final observation")
    if final_status:
        if status.count < previous.count:
            raise ValueError("callback/render count is not monotonic")
        if status.offset_nanoseconds < previous.offset_nanoseconds:
            raise ValueError("local offset is not monotonic")
    else:
        if status.elapsed_seconds <= previous.elapsed_seconds:
            raise ValueError("periodic status elapsed time did not advance")
        if status.count <= previous.count:
            raise ValueError("periodic callback/render count did not advance")
        if status.offset_nanoseconds <= previous.offset_nanoseconds:
            raise ValueError("periodic local offset did not advance")
    if status.maximum_frames < previous.maximum_frames:
        raise ValueError("maximum frame observation is not monotonic")
    if status.maximum_gap_nanoseconds < previous.maximum_gap_nanoseconds:
        raise ValueError("maximum gap observation is not monotonic")


def read_lines(path: Path) -> list[str]:
    return path.read_text(encoding="utf-8").splitlines()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=sorted(BACKENDS), required=True)
    parser.add_argument("log", type=Path)
    arguments = parser.parse_args()

    try:
        report = validate_report(read_lines(arguments.log), arguments.backend)
    except (OSError, ValueError) as error:
        print(f"output soak log: invalid: {error}", file=sys.stderr)
        return 1

    final = report.final_status
    print(
        "output soak log: ok "
        f"backend={BACKENDS[arguments.backend].name} "
        f"duration_s={report.duration_seconds} "
        f"periodic_statuses={len(report.periodic_statuses)} "
        f"final_count={final.count} "
        f"max_frames={final.maximum_frames} "
        f"max_gap_ns={final.maximum_gap_nanoseconds}"
    )
    return 0


class OutputSoakLogTests(unittest.TestCase):
    def test_accepts_completed_cpal_transcript(self) -> None:
        report = validate_report(
            [
                "soak duration (s): 15",
                "report interval (s): 5",
                "default format: 48000 Hz, 8 channels, F32",
                "requested frames: 480",
                "maximum frames: 8192",
                cpal_status(5, 500),
                cpal_status(10, 1_000),
                cpal_status(15, 1_500, maximum_gap_nanoseconds=18_000_000),
                f"final {cpal_status(15, 1_500, maximum_gap_nanoseconds=18_000_000)}",
            ],
            "cpal",
        )
        self.assertEqual(report.duration_seconds, 15)
        self.assertEqual(report.requested_frames, "480")
        self.assertEqual(report.maximum_frames, 8_192)
        self.assertEqual(len(report.periodic_statuses), 3)
        self.assertEqual(report.final_status.maximum_frames, 1056)

    def test_rejects_missing_periodic_status_and_unhealthy_final(self) -> None:
        transcript = [
            "soak duration (s): 15",
            "report interval (s): 5",
            "default format: 48000 Hz, 8 channels, Float32",
            "requested frames: 480",
            "maximum frames: 8192",
            wasapi_status(5, 500),
            f"final {wasapi_status(15, 1_500, errors=1, health='Faulted')}",
        ]
        with self.assertRaisesRegex(ValueError, "missing or extra periodic statuses"):
            validate_report(transcript, "wasapi")

    def test_rejects_decreasing_cpal_callback_count(self) -> None:
        transcript = [
            "soak duration (s): 10",
            "report interval (s): 5",
            "default format: 48000 Hz, 8 channels, F32",
            "requested frames: 480",
            "maximum frames: 8192",
            cpal_status(5, 500),
            cpal_status(10, 400),
            f"final {cpal_status(10, 400)}",
        ]
        with self.assertRaisesRegex(ValueError, "periodic callback/render count did not advance"):
            validate_report(transcript, "cpal")

    def test_rejects_periodic_status_without_callback_progress(self) -> None:
        transcript = [
            "soak duration (s): 10",
            "report interval (s): 5",
            "default format: 48000 Hz, 8 channels, F32",
            "requested frames: 480",
            "maximum frames: 8192",
            cpal_status(5, 500),
            cpal_status(10, 500),
            f"final {cpal_status(10, 500)}",
        ]
        with self.assertRaisesRegex(ValueError, "periodic callback/render count did not advance"):
            validate_report(transcript, "cpal")

    def test_rejects_periodic_status_before_its_interval(self) -> None:
        transcript = [
            "soak duration (s): 10",
            "report interval (s): 5",
            "default format: 48000 Hz, 8 channels, F32",
            "requested frames: 480",
            "maximum frames: 8192",
            cpal_status(1, 100),
            cpal_status(2, 200),
            f"final {cpal_status(10, 1_000)}",
        ]
        with self.assertRaisesRegex(ValueError, "before its configured report interval"):
            validate_report(transcript, "cpal")

    def test_rejects_missing_configuration_and_capacity_violation(self) -> None:
        missing_configuration = [
            "soak duration (s): 5",
            "report interval (s): 5",
            cpal_status(5, 500),
            f"final {cpal_status(5, 500)}",
        ]
        with self.assertRaisesRegex(ValueError, "missing default format"):
            validate_report(missing_configuration, "cpal")

        capacity_violation = [
            "soak duration (s): 5",
            "report interval (s): 5",
            "default format: 48000 Hz, 8 channels, F32",
            "requested frames: default",
            "maximum frames: 1000",
            cpal_status(5, 500),
            f"final {cpal_status(5, 500)}",
        ]
        with self.assertRaisesRegex(ValueError, "exceeds declared maximum"):
            validate_report(capacity_violation, "cpal")

        invalid_request = [
            "soak duration (s): 5",
            "report interval (s): 5",
            "default format: 48000 Hz, 8 channels, F32",
            "requested frames: 1001",
            "maximum frames: 1000",
            cpal_status(5, 500),
            f"final {cpal_status(5, 500)}",
        ]
        with self.assertRaisesRegex(ValueError, "requested frames header exceeds"):
            validate_report(invalid_request, "cpal")

    def test_rejects_invalid_wasapi_endpoint_priming_observation(self) -> None:
        transcript = [
            "soak duration (s): 5",
            "report interval (s): 5",
            "default format: 48000 Hz, 8 channels, Float32",
            "requested frames: 480",
            "maximum frames: 8192",
            wasapi_status(5, 500, endpoint_buffer_frames=1056, primed_frames=1000),
            f"final {wasapi_status(5, 500)}",
        ]
        with self.assertRaisesRegex(ValueError, "priming frames do not match"):
            validate_report(transcript, "wasapi")


def cpal_status(
    elapsed_seconds: int,
    callbacks: int,
    maximum_gap_nanoseconds: int = 17_000_000,
) -> str:
    return (
        f"elapsed_s={elapsed_seconds}, callbacks={callbacks}, latest_frames=480, "
        "max_frames=1056, "
        f"latest_offset_ns={elapsed_seconds * 1_000_000_000}, "
        f"max_gap_ns={maximum_gap_nanoseconds}, oversized=0, errors=0, health=Healthy"
    )


def wasapi_status(
    elapsed_seconds: int,
    renders: int,
    errors: int = 0,
    health: str = "Healthy",
    endpoint_buffer_frames: int = 1056,
    primed_frames: int = 1056,
) -> str:
    return (
        f"elapsed_s={elapsed_seconds}, renders={renders}, latest_frames=480, "
        "max_frames=480, "
        f"latest_offset_ns={elapsed_seconds * 1_000_000_000}, "
        "max_gap_ns=17000000, "
        f"endpoint_buffer_frames={endpoint_buffer_frames}, primed_frames={primed_frames}, "
        f"oversized=0, errors={errors}, health={health}"
    )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
