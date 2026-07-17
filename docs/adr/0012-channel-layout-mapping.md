# ADR 0012: Bounded channel layout mapping

- Status: Provisional
- Date: 2026-07-17

## Context

The Android capture path currently reports a platform-observed mono or stereo
layout, while Windows output devices can expose up to eight channels. The
realtime source contract consumes interleaved Float32 samples and must not
allocate or discover device layout from its callback. Passing stereo samples
directly to an eight-channel callback would reinterpret frame boundaries and
produce incorrect output.

The repository does not yet have qualified speaker-position metadata for every
Android source or Windows endpoint. A semantic 5.1/7.1 speaker map would be an
unverified product claim at this stage.

## Decision

Add a bounded `ChannelMapper` in `sonimesh-audio` and a
`ChannelMappedRenderSource` adapter in `sonimesh-app`.

- both layouts are restricted to one through eight channels;
- identical layouts copy interleaved frames without transformation;
- mono input is duplicated to every output channel;
- a smaller input layout is copied by index and the remaining output channels
  are initialized to silence;
- a larger input layout is folded by output-index modulo and each destination
  receives the average of its contributing input channels;
- the mapper rejects frame-unaligned input and insufficient output storage;
- all source scratch storage is allocated before stream start;
- the realtime adapter initializes its complete output, performs only bounded
  copies and arithmetic, and emits silence for an out-of-bound callback;
- `RenderSourceFactory` receives the selected `OutputConfig`, allowing the
  control-domain factory to install this adapter with the negotiated source
  layout and selected device layout.

This is an index-based compatibility policy, not a claim that channel index
zero is a particular physical speaker. A future speaker-aware policy may replace
the mapper behind the same source boundary after device evidence is available.

## Reliability and realtime implications

The channel count and maximum callback frame count are validated before any
realtime source is constructed. Mapping uses no heap allocation, locks, waits,
network access, device enumeration, or UI calls from `RenderSource::render`.
The fold-down average prevents simple channel-count reduction from multiplying
amplitude, but it does not replace a content-specific limiter or speaker-aware
mix matrix.

The mapper does not resample. Sample-rate conversion and drift correction remain
separate audio-domain stages.

## Validation evidence

- deterministic tests cover identity, mono expansion, zero extension, fold-down,
  invalid layouts, frame alignment, output capacity, and callback tails;
- the Windows CPAL smoke observed an eight-channel endpoint, which motivated
  this boundary, but did not qualify its physical speaker positions;
- real Android channel layouts and physical output channel routing remain
  hardware-in-the-loop evidence gates.

## Rollback plan

Remove the adapter and keep the platform-neutral `RenderSource` contract if
device evidence shows that the policy is unsuitable. The decoded bus and
platform output adapter remain independent, so a future explicit speaker matrix
can replace the provisional mapper without changing network or protocol code.
