# Documentation map

SoniMesh is a feasibility project. A buildable foundation and isolated platform
experiments exist; they do not establish a complete Android-to-Windows product.
Start with the [implementation baseline](./mvp-baseline.md) for the current
snapshot and its explicit gaps.

## Find the right document

| Question | Start here |
| --- | --- |
| What is in scope and how are components separated? | [Architecture](./architecture.md) |
| What is implemented, bounded, or still missing? | [MVP implementation baseline](./mvp-baseline.md) |
| How do trust, pairing, keys, and packet security work? | [Security model](./security-model.md) |
| How do I build, check, or review a change? | [Development](./development.md) |
| What must be measured and how? | [Testing and measurement](./testing-and-measurement.md) |
| What evidence exists for a specific platform experiment? | [Measurement index](./measurements/README.md) |
| What decisions and evidence gates apply? | [ADR index](./adr/README.md) |
| Where are the wire examples? | [Protocol vectors](./protocol/README.md) |
| What are the phase scopes and exit criteria? | [Roadmap](./roadmap.md) |

Component-specific instructions live with the [Android prototype](../apps/android/README.md),
[desktop UI preview](../apps/desktop-ui/README.md), [network simulator](../tools/network-simulator/README.md),
and [latency tool](../tools/latency-measurement/README.md).

## How to interpret and maintain these documents

- Architecture and security documents define intended boundaries; an ADR records
  a particular decision, its status, alternatives, evidence, and remaining gates.
  A provisional decision is not a release qualification.
- The baseline is an implementation snapshot. Check code and tests when making a
  change; update the snapshot when an implemented boundary or gap changes.
- Measurement protocols describe a procedure; dated records describe individual
  runs and their environment. A host build, simulation, or isolated device run
  does not prove an integrated path, hardware latency, or broad compatibility.
- The roadmap defines phase scope and exit criteria, not a live task list. Update
  it when those boundaries change, not for every implementation step.
- Put a definition in its owning document and link to it elsewhere. When changing
  protocol, security, timing, platform support, or compatibility, update the
  relevant design and ADR together. Preserve the distinction between observed
  results, intended behavior, and untested hypotheses.
- Use the checks in [Development](./development.md), including relative-link and
  date checks. These checks do not validate external links, anchor fragments,
  factual claims, or the completeness of measurement evidence.

Optional machine-local notes are discoverable through `.dev/INDEX.md` when
present. They are working context, not a substitute for tracked decisions or
evidence; never put secrets in them.
