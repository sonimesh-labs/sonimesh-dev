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
HEADER_PATTERNS = {
    "cycles": re.compile(r"^cycles: (?P<value>\d+)$"),
    "active duration": re.compile(r"^active duration \(ms\): (?P<value>\d+)$"),
}


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
    statuses: tuple[CycleStatus, ...]


def parse_header(line: str) -> tuple[str, int] | None:
    for name, pattern in HEADER_PATTERNS.items():
        match = pattern.fullmatch(line)
        if match is not None:
            return name, int(match.group("value"))
    return None


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
    statuses: list[CycleStatus] = []
    for line in lines:
        header = parse_header(line)
        if header is not None:
            name, value = header
            if name in headers:
                raise ValueError(f"duplicate {name} header")
            headers[name] = value
            continue
        status = parse_status(line, spec)
        if status is not None:
            statuses.append(status)

    cycles = headers.get("cycles")
    active_duration = headers.get("active duration")
    if cycles is None or active_duration is None:
        raise ValueError("missing cycles or active duration header")
    if not 1 <= cycles <= MAXIMUM_CYCLES:
        raise ValueError("cycle count is outside the accepted command bound")
    if not 1 <= active_duration <= MAXIMUM_ACTIVE_MILLIS:
        raise ValueError("active duration is outside the accepted command bound")
    if len(statuses) != cycles:
        raise ValueError(f"missing or extra cycle statuses: expected {cycles}, got {len(statuses)}")

    for expected_cycle, status in enumerate(statuses, start=1):
        if status.cycle != expected_cycle:
            raise ValueError(
                f"cycle status sequence is not contiguous: expected {expected_cycle}, got {status.cycle}"
            )
        validate_status(status, spec)
    return LifecycleReport(cycles, active_duration, tuple(statuses))


def validate_status(status: CycleStatus, spec: BackendSpec) -> None:
    if status.count <= 0 or status.offset_nanoseconds <= 0:
        raise ValueError(f"cycle {status.cycle} has no observed {spec.count_key} work")
    if status.latest_frames <= 0 or status.maximum_frames < status.latest_frames:
        raise ValueError(f"cycle {status.cycle} has invalid observed frame bounds")
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
                cpal_status(1, "480", 25, 1_056),
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
            cpal_status(1, "480", 25, 480),
            cpal_status(3, "480", 25, 480),
        ]
        with self.assertRaisesRegex(ValueError, "not contiguous"):
            validate_report(transcript, "cpal")

        transcript[3] = cpal_status(2, "480", 25, 100, latest_frames=480)
        with self.assertRaisesRegex(ValueError, "invalid observed frame bounds"):
            validate_report(transcript, "cpal")


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


def wasapi_status(cycle: int, renders: int, errors: int = 0, health: str = "Healthy") -> str:
    return (
        f"cycle {cycle}: request=480, renders={renders}, latest_frames=480, max_frames=480, "
        f"latest_offset_ns={cycle * 250_000_000}, max_gap_ns=17000000, primed_frames=1056, "
        f"oversized=0, errors={errors}, health={health}"
    )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
