# Architecture

loctel receives OTLP traces and logs, assembles them in memory, works out
which service failed, and serves the result to a menu bar UI and to AI agents
over MCP. The design is in
[`superpowers/specs/2026-10-21-loctel-design.md`](superpowers/specs/2026-10-21-loctel-design.md).
This page is the map: update it whenever a crate is added or a data path
changes.

The hard requirement (design §11): **the core has no UI or graphics
dependency and builds and tests on Linux alone.** The UI stays thin over it,
and macOS-only code sits behind `cfg(target_os = "macos")` in its own small
crate.

## Crate map

Status says what is in the repository now. Rows marked "planned" are the
Milestone 1 target from `superpowers/plans/2026-10-21-loctel-m1-core.md`;
the task that creates a crate changes its row.

| Crate | Layer | Owns | Status |
|---|---|---|---|
| `loctel-model` | core | Domain types (spans, logs, resources, IDs, severity), independent of OTLP | placeholder (one test) |
| `loctel-store` | core | In-memory store: trace assembly, completeness, dedup, eviction, failure semantics, search, logs, service map | planned |
| `loctel-ingest` | core | OTLP gRPC and HTTP receivers, protobuf and JSON to model conversion | planned |
| `loctel-mcp` | core | MCP server (rmcp, Streamable HTTP), tools and their DTOs | planned |
| `loctel-engine` | core | Wires store, ingest and MCP; settings, pause, rates; owns threads and runtime | planned |
| `loctel-meshgen` | test tooling | Fake microservice mesh scenarios (library and `meshgen` binary) | planned |
| `loctel-e2e` | test tooling | End-to-end tests through real OTLP and a real MCP client (`publish = false`) | planned |
| `loctel-ui` | UI | GPUI views and the headless screenshot harness | planned |
| `loctel-macos` | macOS | Status item, activation policy, login item (`cfg(target_os = "macos")`) | planned |
| `loctel-app` | binary | The `loctel` binary: headless mode and GUI, bundle metadata | planned |
| `xtask` | tooling | Screenshots, next-version, release notes, MCP smoke test | placeholder (empty `main`) |

## Data flow

Target shape for Milestone 1:

```
 services (Docker or bare metal)
        |  OTLP gRPC :4317, OTLP HTTP :4318 (protobuf or JSON, gzip)
        v
 loctel-ingest ---- proto -> loctel-model types, invalid items -> partial_success
        |
        v
 loctel-store  ---- assembles traces, marks complete, finds failed-in span,
        |           keeps logs and the service map, evicts oldest whole trace
        v
 loctel-engine ---- settings, pause, rates; owns the runtime
     |         |
     v         v
 loctel-mcp   loctel-ui  (status view, menu text)  <-- loctel-macos (status item)
     |
     v
 AI agents  (MCP 2026-07-28, http://127.0.0.1:4319/mcp, read-only)
```

Telemetry is never persisted. Only `settings.toml` (and discard rules from
Milestone 2) is written to disk.

## Platform

macOS 26 or later on Apple Silicon is the product. Linux x86_64 exists for
development and tests only. Mac-only features are unverified until a milestone
check on a real Mac (design §10.4).
