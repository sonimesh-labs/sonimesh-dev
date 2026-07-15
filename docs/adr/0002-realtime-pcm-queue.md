# ADR 0002: Realtime PCM queue boundary

- Status: Accepted for single-stream feasibility work
- Date: 2026-07-15

## Context

Decoded PCM must cross from a per-stream worker into the platform output callback.
The callback cannot allocate, lock, wait, decode, or access platform control APIs.
When the producer outruns output, the system must remain bounded, discard old
audio rather than build latency, and make the discontinuity observable for fade
handling.

The workspace forbids project-owned unsafe code. A custom concurrent ring would
require unsafe interior mutability and a separate memory-model verification effort.
`std::sync::mpsc::sync_channel` does not provide a documented lock-free realtime
contract. `rtrb` provides SPSC behavior, but its producer cannot atomically replace
the oldest item when full.

## Decision

Use `crossbeam_queue::ArrayQueue<PcmBlock>` behind non-cloneable logical producer
and consumer endpoints in `sonimesh-audio`.

- allocation and `Arc` creation occur only when the stream queue is constructed;
- queue capacity is non-zero and capped at 128 blocks;
- every PCM block stores samples inline with a compile-time sample limit;
- `force_push` atomically replaces the oldest block when full;
- producer-local epoch and sequence markers let the consumer detect every gap;
- callback reads always initialize their complete output slice, using silence on
  underflow;
- the public SoniMesh implementation contains no unsafe block.

Although `ArrayQueue` is an MPMC implementation, SoniMesh creates exactly one
non-cloneable producer wrapper and one non-cloneable consumer wrapper. The wider
algorithm is used only to support producer-side oldest replacement safely.

## Security and realtime implications

Capacity validation prevents unbounded memory use. PCM blocks contain no key or
network identity. Queue methods are lock-free and do not allocate after
construction. Destruction and output-stream recreation remain control-domain work.

`crossbeam-queue` becomes a runtime dependency and must remain covered by supply-
chain review. A future platform benchmark must still verify callback timing on
actual WASAPI devices; lock-free does not by itself prove a latency target.

## Validation evidence

Tests cover invalid capacities, oldest replacement, stream-epoch discontinuity,
underflow initialization, and a 50,000-block concurrent producer/consumer stress
run. The stress test proves ordering and accounts every block as consumed or
overwritten; it is not a substitute for Loom, Miri, or hardware timing evidence.

## Rollback plan

The dependency is hidden behind SoniMesh-owned endpoints. It can be replaced by
another reviewed queue without changing decoder, mixer, platform, or UI APIs.

