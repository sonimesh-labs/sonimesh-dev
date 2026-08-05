#!/usr/bin/env python3
"""Validate retained Windows output lifecycle-smoke control-domain transcripts."""

from __future__ import annotations

import argparse
import re
import sys
import unittest
from dataclasses import dataclass
from pathlib import Path


MAXIMUM_CYCLES = 500
MAXIMUM_ACTIVE_MILLIS = 1_000
MINIMUM_CALLBACK_FRAMES = 16
MAXIMUM_CALLBACK_FRAMES = 8_192
FIXED_REQUEST_FRAMES = 480
HEADER_PATTERNS = {
    "cycles": re.compile(r"^cycles: (?P<value>\d+)$"),
    "active duration": re.compile(r"^active duration \(ms\): (?P<value>\d+)$"),
}
MAXIMUM_FRAMES_PATTERN = re.compile(r"^maximum frames: (?P<value>\d+)$")
FIXED_REQUEST_REJECTION_PATTERN = re.compile(
    r"^cycle (?P<cycle>\d+): fixed 480-frame request rejected: .+$"
)
FORMAT_PREFIX = "default format: "


@dataclass(frozen=True)
class BackendSpec:
    name: str
    count_key: str
    fields: tuple[str, ...]


BACKENDS = {
    "cpal": BackendSpec(
        name="CPAL",
        count_key="callbacks",
        fields=(
            "request",
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
        fields=(
            "request",
            "renders",
            "latest_frames",
            "max_frames",
            "latest_offset_ns",
            "max_gap_ns",
            "primed_frames",
            "oversized",
            "errors",
            "health",
        ),
    ),
}


@dataclass(frozen=True)
class CycleStatus:
    cycle: int
    request: str
    count: int
    latest_frames: int
    maximum_frames: int
    offset_nanoseconds: int
    maximum_gap_nanoseconds: int
    oversized: int
    errors: int
    health: str


@dataclass(frozen=True)
class LifecycleReport:
    cycles: int
    active_duration_millis: int
    default_format: str
    maximum_frames: int
    statuses: tuple[CycleStatus, ...]


def parse_header(line: str) -> tuple[str, int] | None:
    for name, pattern in HEADER_PATTERNS.items():
        match = pattern.fullmatch(line)
        if match is not None:
            return name, int(match.group("value"))
    return None


def parse_configuration_header(line: str) -> tuple[str, str | int] | None:
    if line.startswith(FORMAT_PREFIX):
        value = line.removeprefix(FORMAT_PREFIX)
        if not value.strip():
            raise ValueError("default format header is empty")
        return "default format", value
    maximum = MAXIMUM_FRAMES_PATTERN.fullmatch(line)
    if maximum is not None:
        return "maximum frames", int(maximum.group("value"))
    return None


def parse_fixed_request_rejection(line: str) -> int | None:
    match = FIXED_REQUEST_REJECTION_PATTERN.fullmatch(line)
    return int(match.group("cycle")) if match is not None else None


def parse_status(line: str, spec: BackendSpec) -> CycleStatus | None:
    match = re.match(r"^cycle (?P<cycle>\d+): (?P<fields>.*)$", line)
    if match is None:
        return None

    values: dict[str, str] = {}
    for field in match.group("fields").split(", "):
        key, separator, value = field.partition("=")
        if not separator or not key or not value or key in values:
            raise ValueError(f"malformed {spec.name} cycle field: {field!r}")
        values[key] = value
    if tuple(values) != spec.fields:
        raise ValueError(
            f"unexpected {spec.name} cycle layout: expected {', '.join(spec.fields)}"
        )
    request = values["request"]
    if request != "default" and (not request.isdecimal() or int(request) <= 0):
        raise ValueError("request must be default or a positive frame count")

    try:
        return CycleStatus(
            cycle=int(match.group("cycle")),
            request=request,
            count=int(values[spec.count_key]),
            latest_frames=int(values["latest_frames"]),
            maximum_frames=int(values["max_frames"]),
            offset_nanoseconds=int(values["latest_offset_ns"]),
            maximum_gap_nanoseconds=int(values["max_gap_ns"]),
            oversized=int(values["oversized"]),
            errors=int(values["errors"]),
            health=values["health"],
        )
    except ValueError as error:
        raise ValueError(f"invalid {spec.name} numeric cycle field") from error


def validate_report(lines: list[str], backend: str) -> LifecycleReport:
    spec = BACKENDS[backend]
    headers: dict[str, int] = {}
    configuration: dict[str, str | int] = {}
    fixed_request_rejections: dict[int, int] = {}
    statuses: list[tuple[int, CycleStatus]] = []
    for line_number, line in enumerate(lines, start=1):
        header = parse_header(line)
        if header is not None:
            name, value = header
            if name in headers:
                raise ValueError(f"duplicate {name} header")
            headers[name] = value
            continue
        configuration_header = parse_configuration_header(line)
        if configuration_header is not None:
            name, value = configuration_header
            if name in configuration:
                raise ValueError(f"duplicate {name} header")
            configuration[name] = value
            continue
        fixed_request_rejection = parse_fixed_request_rejection(line)
        if fixed_request_rejection is not None:
            if fixed_request_rejection in fixed_request_rejections:
                raise ValueError(f"duplicate fixed-request rejection for cycle {fixed_request_rejection}")
            fixed_request_rejections[fixed_request_rejection] = line_number
            continue
        status = parse_status(line, spec)
        if status is not None:
            statuses.append((line_number, status))

    cycles = headers.get("cycles")
    active_duration = headers.get("active duration")
    default_format = configuration.get("default format")
    maximum_frames = configuration.get("maximum frames")
    if cycles is None or active_duration is None:
        raise ValueError("missing cycles or active duration header")
    if not isinstance(default_format, str) or not isinstance(maximum_frames, int):
        raise ValueError("missing default format or maximum frames header")
    if not 1 <= cycles <= MAXIMUM_CYCLES:
        raise ValueError("cycle count is outside the accepted command bound")
    if not 1 <= active_duration <= MAXIMUM_ACTIVE_MILLIS:
        raise ValueError("active duration is outside the accepted command bound")
    if not MINIMUM_CALLBACK_FRAMES <= maximum_frames <= MAXIMUM_CALLBACK_FRAMES:
        raise ValueError("maximum frames header is outside the accepted callback bound")
    if len(statuses) != cycles:
        raise ValueError(f"missing or extra cycle statuses: expected {cycles}, got {len(statuses)}")
    if any(cycle < 1 or cycle > cycles for cycle in fixed_request_rejections):
        raise ValueError("fixed-request rejection is outside the configured cycle range")

    consumed_fixed_request_rejections: set[int] = set()
    previous_status_line = 0
    for expected_cycle, (status_line, status) in enumerate(statuses, start=1):
        if status.cycle != expected_cycle:
            raise ValueError(
                f"cycle status sequence is not contiguous: expected {expected_cycle}, got {status.cycle}"
            )
        rejection_line = fixed_request_rejections.get(status.cycle)
        fixed_request_rejected = (
            rejection_line is not None and previous_status_line < rejection_line < status_line
        )
        validate_status(status, spec, maximum_frames, fixed_request_rejected)
        if status.request == "default" and fixed_request_rejected:
            consumed_fixed_request_rejections.add(status.cycle)
        previous_status_line = status_line
    if len(consumed_fixed_request_rejections) != len(fixed_request_rejections):
        raise ValueError("fixed-request rejection is not paired with a following same-cycle default request")
    return LifecycleReport(
        cycles,
        active_duration,
        default_format,
        maximum_frames,
        tuple(status for _, status in statuses),
    )


def validate_status(
    status: CycleStatus,
    spec: BackendSpec,
    maximum_frames: int,
    fixed_request_rejected: bool,
) -> None:
    if status.count <= 0 or status.offset_nanoseconds <= 0:
        raise ValueError(f"cycle {status.cycle} has no observed {spec.count_key} work")
    if status.latest_frames <= 0 or status.maximum_frames < status.latest_frames:
        raise ValueError(f"cycle {status.cycle} has invalid observed frame bounds")
    if status.maximum_frames > maximum_frames:
        raise ValueError(f"cycle {status.cycle} observed maximum frames exceed the declared bound")
    if status.request == "default":
        if not fixed_request_rejected:
            raise ValueError(
                f"cycle {status.cycle} default request did not retain a fixed-request rejection"
            )
    else:
        if fixed_request_rejected:
            raise ValueError(
                f"cycle {status.cycle} fixed request cannot retain a fallback rejection"
            )
        if status.request != str(FIXED_REQUEST_FRAMES):
            raise ValueError(
                f"cycle {status.cycle} request must be fixed {FIXED_REQUEST_FRAMES} frames or default"
            )
        if FIXED_REQUEST_FRAMES > maximum_frames:
            raise ValueError(f"cycle {status.cycle} fixed request exceeds the declared callback bound")
    if status.maximum_gap_nanoseconds < 0:
        raise ValueError(f"cycle {status.cycle} has an invalid observed gap")
    if status.oversized != 0 or status.errors != 0 or status.health != "Healthy":
        raise ValueError(
            f"cycle {status.cycle} is unhealthy: oversized={status.oversized}, "
            f"errors={status.errors}, health={status.health}"
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend", choices=sorted(BACKENDS), required=True)
    parser.add_argument("log", type=Path)
    arguments = parser.parse_args()
    try:
        report = validate_report(arguments.log.read_text(encoding="utf-8").splitlines(), arguments.backend)
    except (OSError, ValueError) as error:
        print(f"output lifecycle log: invalid: {error}", file=sys.stderr)
        return 1

    observed_maximum = max(status.maximum_frames for status in report.statuses)
    requests = ",".join(sorted({status.request for status in report.statuses}))
    print(
        "output lifecycle log: ok "
        f"backend={BACKENDS[arguments.backend].name} "
        f"cycles={report.cycles} "
        f"active_duration_ms={report.active_duration_millis} "
        f"format={report.default_format!r} "
        f"declared_max_frames={report.maximum_frames} "
        f"requests={requests} "
        f"max_frames={observed_maximum}"
    )
    return 0


class LifecycleLogTests(unittest.TestCase):
    def test_accepts_completed_cpal_transcript(self) -> None:
        report = validate_report(
            [
                "cycles: 2",
                "active duration (ms): 250",
                "default format: 48000 Hz, 8 channels, F32",
                "maximum frames: 8192",
                cpal_status(1, "480", 25, 1_056),
                fixed_request_rejection(2),
                cpal_status(2, "default", 24, 960),
            ],
            "cpal",
        )
        self.assertEqual(report.cycles, 2)
        self.assertEqual(report.statuses[1].request, "default")

    def test_rejects_missing_cycle_and_unhealthy_status(self) -> None:
        transcript = [
            "cycles: 2",
            "active duration (ms): 250",
            "default format: 48000 Hz, 8 channels, Float32",
            "maximum frames: 8192",
            wasapi_status(1, 25),
        ]
        with self.assertRaisesRegex(ValueError, "missing or extra cycle statuses"):
            validate_report(transcript, "wasapi")

        transcript.append(wasapi_status(2, 25, errors=1, health="Faulted"))
        with self.assertRaisesRegex(ValueError, "cycle 2 is unhealthy"):
            validate_report(transcript, "wasapi")

    def test_rejects_noncontiguous_cycle_and_invalid_frames(self) -> None:
        transcript = [
            "cycles: 2",
            "active duration (ms): 250",
            "default format: 48000 Hz, 8 channels, F32",
            "maximum frames: 8192",
            cpal_status(1, "480", 25, 480),
            cpal_status(3, "480", 25, 480),
        ]
        with self.assertRaisesRegex(ValueError, "not contiguous"):
            validate_report(transcript, "cpal")

        transcript[5] = cpal_status(2, "480", 25, 100, latest_frames=480)
        with self.assertRaisesRegex(ValueError, "invalid observed frame bounds"):
            validate_report(transcript, "cpal")


    def test_requires_bounded_declared_configuration(self) -> None:
        base = [
            "cycles: 1",
            "active duration (ms): 250",
            cpal_status(1, "480", 25, 480),
        ]
        with self.assertRaisesRegex(ValueError, "missing default format or maximum frames"):
            validate_report(base, "cpal")

        with self.assertRaisesRegex(ValueError, "default format header is empty"):
            validate_report(
                [
                    "cycles: 1",
                    "active duration (ms): 250",
                    "default format: ",
                    "maximum frames: 8192",
                    cpal_status(1, "480", 25, 480),
                ],
                "cpal",
            )

        with self.assertRaisesRegex(ValueError, "maximum frames header is outside"):
            validate_report(
                [
                    "cycles: 1",
                    "active duration (ms): 250",
                    "default format: 48000 Hz, 8 channels, F32",
                    "maximum frames: 8193",
                    cpal_status(1, "480", 25, 480),
                ],
                "cpal",
            )

    def test_requires_a_recorded_fixed_request_rejection_for_default_fallback(self) -> None:
        headers = [
            "cycles: 1",
            "active duration (ms): 250",
            "default format: 48000 Hz, 8 channels, Float32",
            "maximum frames: 8192",
        ]
        with self.assertRaisesRegex(ValueError, "default request did not retain a fixed-request rejection"):
            validate_report([*headers, wasapi_status(1, 25, request="default")], "wasapi")
        with self.assertRaisesRegex(ValueError, "default request did not retain a fixed-request rejection"):
            validate_report(
                [*headers, wasapi_status(1, 25, request="default"), fixed_request_rejection(1)],
                "wasapi",
            )

        report = validate_report(
            [*headers, fixed_request_rejection(1), wasapi_status(1, 25, request="default")],
            "wasapi",
        )
        self.assertEqual(report.statuses[0].request, "default")

        future_cycle_headers = [
            "cycles: 2",
            "active duration (ms): 250",
            "default format: 48000 Hz, 8 channels, F32",
            "maximum frames: 8192",
        ]
        with self.assertRaisesRegex(ValueError, "default request did not retain a fixed-request rejection"):
            validate_report(
                [
                    *future_cycle_headers,
                    fixed_request_rejection(2),
                    cpal_status(1, "480", 25, 480),
                    cpal_status(2, "default", 25, 480),
                ],
                "cpal",
            )

        with self.assertRaisesRegex(ValueError, "fixed request cannot retain a fallback rejection"):
            validate_report(
                [*headers, fixed_request_rejection(1), wasapi_status(1, 25)],
                "wasapi",
            )
        with self.assertRaisesRegex(ValueError, "not paired with a following same-cycle default"):
            validate_report(
                [*headers, wasapi_status(1, 25), fixed_request_rejection(1)],
                "wasapi",
            )

    def test_rejects_cycle_frames_or_requests_outside_the_command_contract(self) -> None:
        limited_headers = [
            "cycles: 1",
            "active duration (ms): 250",
            "default format: 48000 Hz, 8 channels, F32",
            "maximum frames: 479",
        ]
        with self.assertRaisesRegex(ValueError, "observed maximum frames exceed"):
            validate_report([*limited_headers, cpal_status(1, "480", 25, 480)], "cpal")
        with self.assertRaisesRegex(ValueError, "fixed request exceeds the declared callback bound"):
            validate_report(
                [*limited_headers, cpal_status(1, "480", 25, 479, latest_frames=479)],
                "cpal",
            )
        with self.assertRaisesRegex(ValueError, "observed maximum frames exceed"):
            validate_report(
                [*limited_headers, wasapi_status(1, 25, maximum_frames=480)],
                "wasapi",
            )

        headers = [
            "cycles: 1",
            "active duration (ms): 250",
            "default format: 48000 Hz, 8 channels, F32",
            "maximum frames: 8192",
        ]
        for request in ("16", "479", "481"):
            with self.subTest(request=request):
                with self.assertRaisesRegex(ValueError, "request must be fixed 480 frames or default"):
                    validate_report([*headers, cpal_status(1, request, 25, 480)], "cpal")


def cpal_status(
    cycle: int,
    request: str,
    callbacks: int,
    maximum_frames: int,
    latest_frames: int = 480,
) -> str:
    return (
        f"cycle {cycle}: request={request}, callbacks={callbacks}, latest_frames={latest_frames}, "
        f"max_frames={maximum_frames}, latest_offset_ns={cycle * 250_000_000}, "
        "max_gap_ns=17000000, oversized=0, errors=0, health=Healthy"
    )


def fixed_request_rejection(cycle: int) -> str:
    return f"cycle {cycle}: fixed 480-frame request rejected: test fallback"


def wasapi_status(
    cycle: int,
    renders: int,
    errors: int = 0,
    health: str = "Healthy",
    maximum_frames: int = 480,
    request: str = "480",
) -> str:
    return (
        f"cycle {cycle}: request={request}, renders={renders}, latest_frames=480, "
        f"max_frames={maximum_frames}, "
        f"latest_offset_ns={cycle * 250_000_000}, max_gap_ns=17000000, primed_frames=1056, "
        f"oversized=0, errors={errors}, health={health}"
    )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
