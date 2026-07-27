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
- producer publishes an atomic active epoch before emitting new PCM blocks;
  consumer-side epoch and sequence markers let the callback discard stale blocks
  and detect every remaining gap; public active-epoch occupancy is always clamped
  to the configured queue capacity during concurrent oldest replacement;
- callback reads always initialize their complete output slice, using silence on
  underflow or when only stale blocks were available; exhausting the fixed
  128-pop refill budget emits silence and increments a distinct atomic metric;
- the queue-backed render source turns an observed epoch or sequence gap into a
  fixed 128-sample fade-in before returning to full PCM, without allocation or
  controller work in the callback;
- output recovery creates a fresh producer/consumer pair and render source
  together; the stream owner installs the new producer only after stopping the
  old platform callback.
- the public SoniMesh implementation contains no unsafe block.

Although `ArrayQueue` is an MPMC implementation, SoniMesh creates exactly one
non-cloneable producer wrapper and one non-cloneable consumer wrapper. The wider
algorithm is used only to support producer-side oldest replacement safely.

## Security and realtime implications

Capacity validation prevents unbounded memory use. PCM blocks contain no key or
network identity. Queue methods are lock-free and do not allocate after
construction. Destruction and output-stream recreation remain control-domain work.

After a stream epoch change, the consumer discards only blocks whose epoch no
longer matches the atomic active epoch. The discard loop is explicitly bounded by
the 128-block queue maximum; it emits silence if no current block remains. A
concurrent epoch change can still race with one callback already reading a block,
so the control domain must perform its documented stream-local fade/reset around
the transition. No queue operation affects another stream.

`crossbeam-queue` becomes a runtime dependency and must remain covered by supply-
chain review. A future platform benchmark must still verify callback timing on
actual WASAPI devices; lock-free does not by itself prove a latency target.

## Validation evidence

Tests cover invalid capacities, oldest replacement, stale epoch discard,
underflow initialization, refill-budget exhaustion, capacity-clamped public
occupancy, queue-discontinuity fade-in, and a 50,000-block concurrent
producer/consumer
stress run. The stress test proves ordering and
accounts every block as consumed or overwritten; it is not a substitute for
Loom, Miri, or hardware timing evidence.

## Rollback plan

The dependency is hidden behind SoniMesh-owned endpoints. It can be replaced by
another reviewed queue without changing decoder, mixer, platform, or UI APIs.
