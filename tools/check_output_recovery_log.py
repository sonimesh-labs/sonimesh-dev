#!/usr/bin/env python3
"""Validate retained Windows default-role and pinned-device recovery transcripts."""

from __future__ import annotations

import argparse
import re
import sys
import unittest
from dataclasses import dataclass
from pathlib import Path


@dataclass(frozen=True)
class BackendSpec:
    count_key: str
    fields: tuple[str, ...]


BACKENDS = {
    "cpal": BackendSpec(
        count_key="callbacks",
        fields=(
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
        count_key="renders",
        fields=(
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
STATUS_PATTERN = re.compile(r"^stage=(before_stop|recovered), (?P<fields>.*)$")


@dataclass(frozen=True)
class Status:
    stage: str
    count: int
    latest_frames: int
    maximum_frames: int
    offset_nanoseconds: int
    maximum_gap_nanoseconds: int
    oversized: int
    errors: int
    health: str


@dataclass(frozen=True)
class RecoveryTranscript:
    fields: dict[str, str]
    statuses: dict[str, Status]
    field_lines: dict[str, int]
    status_lines: dict[str, int]


def parse_status(line: str, spec: BackendSpec) -> Status | None:
    """Parses one redacted status line from a recovery experiment."""
    match = STATUS_PATTERN.fullmatch(line)
    if match is None:
        return None
    values: dict[str, str] = {}
    for field in match.group("fields").split(", "):
        key, separator, value = field.partition("=")
        if not separator or not key or not value or key in values:
            raise ValueError(f"malformed recovery status field: {field!r}")
        values[key] = value
    if tuple(values) != spec.fields:
        raise ValueError("unexpected recovery status layout")
    try:
        return Status(
            stage=match.group(1),
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
        raise ValueError("invalid numeric recovery status field") from error


def parse(lines: list[str], backend: str) -> RecoveryTranscript:
    """Collects checked fields, stages, and line order without retaining endpoint data."""
    spec = BACKENDS[backend]
    fields: dict[str, str] = {}
    statuses: dict[str, Status] = {}
    field_lines: dict[str, int] = {}
    status_lines: dict[str, int] = {}
    accepted = {
        "require default change",
        "default changed",
        "pre_stop_healthy",
        "recovery_result",
    }
    for line_number, line in enumerate(lines, start=1):
        status = parse_status(line, spec)
        if status is not None:
            if status.stage in statuses:
                raise ValueError(f"duplicate {status.stage} status")
            statuses[status.stage] = status
            status_lines[status.stage] = line_number
            continue
        key, separator, value = line.partition(": ")
        if not separator:
            key, separator, value = line.partition("=")
        if separator and key in accepted:
            if key in fields:
                raise ValueError(f"duplicate {key} field")
            fields[key] = value
            field_lines[key] = line_number
    return RecoveryTranscript(fields, statuses, field_lines, status_lines)


def validate_healthy(status: Status) -> None:
    """Rejects a status that cannot support a healthy control-domain observation."""
    if status.count <= 0 or status.offset_nanoseconds <= 0:
        raise ValueError(f"{status.stage} has no observed callback/render work")
    if status.latest_frames <= 0 or status.maximum_frames < status.latest_frames:
        raise ValueError(f"{status.stage} has invalid observed frame bounds")
    if status.maximum_gap_nanoseconds < 0:
        raise ValueError(f"{status.stage} has invalid observed gap")
    if status.oversized != 0 or status.errors != 0 or status.health != "Healthy":
        raise ValueError(f"{status.stage} is not healthy")


def validate_default(lines: list[str], backend: str) -> None:
    """Validates one observed default-role transition transcript."""
    transcript = parse(lines, backend)
    if transcript.fields.get("require default change") != "true":
        raise ValueError("default-role transcript did not require a change")
    if transcript.fields.get("default changed") != "true":
        raise ValueError("default-role transcript did not observe a change")
    if set(transcript.statuses) != {"before_stop", "recovered"}:
        raise ValueError("default-role transcript requires before_stop and recovered statuses")
    require_change_line = transcript.field_lines["require default change"]
    before_stop_line = transcript.status_lines["before_stop"]
    default_changed_line = transcript.field_lines["default changed"]
    recovered_line = transcript.status_lines["recovered"]
    if not require_change_line < before_stop_line < default_changed_line < recovered_line:
        raise ValueError("default-role transcript is not in the expected control-domain order")
    for status in transcript.statuses.values():
        validate_healthy(status)


def validate_pinned(lines: list[str], backend: str) -> None:
    """Validates one CPAL pinned-device outcome without upgrading it to a claim."""
    if backend != "cpal":
        raise ValueError("pinned-device verification is CPAL-only")
    transcript = parse(lines, backend)
    before_stop = transcript.statuses.get("before_stop")
    if before_stop is None:
        raise ValueError("pinned transcript requires a before_stop status")
    before_stop_line = transcript.status_lines["before_stop"]
    outcome = transcript.fields.get("recovery_result")
    outcome_line = transcript.field_lines.get("recovery_result")
    if outcome == "pinned_device_unavailable":
        if "recovered" in transcript.statuses or "pre_stop_healthy" in transcript.fields:
            raise ValueError("unavailable transcript contains a restart outcome")
        if outcome_line is None or outcome_line <= before_stop_line:
            raise ValueError("pinned unavailable transcript is not in the expected control-domain order")
        return
    if outcome != "pinned_device_restarted":
        raise ValueError("pinned transcript has no recognized recovery result")
    if transcript.fields.get("pre_stop_healthy") != "true":
        raise ValueError("pinned restart transcript did not retain healthy pre-stop status")
    recovered = transcript.statuses.get("recovered")
    if recovered is None:
        raise ValueError("pinned restart transcript requires a recovered status")
    pre_stop_healthy_line = transcript.field_lines["pre_stop_healthy"]
    recovered_line = transcript.status_lines["recovered"]
    if outcome_line is None or not before_stop_line < recovered_line < pre_stop_healthy_line < outcome_line:
        raise ValueError("pinned restart transcript is not in the expected control-domain order")
    validate_healthy(before_stop)
    validate_healthy(recovered)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", choices=("default", "pinned"), required=True)
    parser.add_argument("--backend", choices=sorted(BACKENDS), required=True)
    parser.add_argument("log", type=Path)
    arguments = parser.parse_args()
    try:
        lines = arguments.log.read_text(encoding="utf-8").splitlines()
        if arguments.scenario == "default":
            validate_default(lines, arguments.backend)
        else:
            validate_pinned(lines, arguments.backend)
    except (OSError, ValueError) as error:
        print(f"output recovery log: invalid: {error}", file=sys.stderr)
        return 1
    print(f"output recovery log: ok scenario={arguments.scenario} backend={arguments.backend}")
    return 0


class RecoveryLogTests(unittest.TestCase):
    def test_accepts_observed_default_role_change(self) -> None:
        validate_default(
            [
                "require default change: true",
                cpal_status("before_stop", 199),
                "default changed: true",
                cpal_status("recovered", 200),
            ],
            "cpal",
        )

    def test_rejects_default_no_change_command_check(self) -> None:
        with self.assertRaisesRegex(ValueError, "did not require a change"):
            validate_default(
                [
                    "require default change: false",
                    cpal_status("before_stop", 199),
                    "default changed: false",
                    cpal_status("recovered", 200),
                ],
                "cpal",
            )

    def test_rejects_default_status_or_change_observation_out_of_order(self) -> None:
        with self.assertRaisesRegex(ValueError, "expected control-domain order"):
            validate_default(
                [
                    "require default change: true",
                    cpal_status("recovered", 200),
                    "default changed: true",
                    cpal_status("before_stop", 199),
                ],
                "cpal",
            )
        with self.assertRaisesRegex(ValueError, "expected control-domain order"):
            validate_default(
                [
                    "require default change: true",
                    cpal_status("before_stop", 199),
                    cpal_status("recovered", 200),
                    "default changed: true",
                ],
                "cpal",
            )

    def test_accepts_pinned_unavailable_and_healthy_restart_only(self) -> None:
        validate_pinned(
            [cpal_status("before_stop", 199), "recovery_result=pinned_device_unavailable"],
            "cpal",
        )
        validate_pinned(
            [
                cpal_status("before_stop", 199),
                cpal_status("recovered", 200),
                "pre_stop_healthy=true",
                "recovery_result=pinned_device_restarted",
            ],
            "cpal",
        )
        with self.assertRaisesRegex(ValueError, "expected control-domain order"):
            validate_pinned(
                [
                    "recovery_result=pinned_device_unavailable",
                    cpal_status("before_stop", 199),
                ],
                "cpal",
            )
        with self.assertRaisesRegex(ValueError, "expected control-domain order"):
            validate_pinned(
                [
                    cpal_status("recovered", 200),
                    cpal_status("before_stop", 199),
                    "pre_stop_healthy=true",
                    "recovery_result=pinned_device_restarted",
                ],
                "cpal",
            )
        with self.assertRaisesRegex(ValueError, "expected control-domain order"):
            validate_pinned(
                [
                    cpal_status("before_stop", 199),
                    cpal_status("recovered", 200),
                    "recovery_result=pinned_device_restarted",
                    "pre_stop_healthy=true",
                ],
                "cpal",
            )
        with self.assertRaisesRegex(ValueError, "CPAL-only"):
            validate_pinned([], "wasapi")


def cpal_status(stage: str, callbacks: int) -> str:
    """Builds one CPAL status line matching the experiment's public output."""
    return (
        f"stage={stage}, callbacks={callbacks}, latest_frames=480, max_frames=1056, "
        f"latest_offset_ns={callbacks * 10_000_000}, max_gap_ns=17000000, "
        "oversized=0, errors=0, health=Healthy"
    )


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.argv.remove("--self-test")
        unittest.main()
    else:
        raise SystemExit(main())
