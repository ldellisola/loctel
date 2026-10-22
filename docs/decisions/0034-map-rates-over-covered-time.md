# 0034: Map rates divide by the time the data covers

Date: 2026-10-22

## Context

The M3 plan said map rates are `count / window_secs`, and the first version
used the whole range as `window_secs`: 900 seconds under the default
`Last 15 minutes`. The whole-branch review found what that means in use: a
loctel that started a minute ago divides a minute of requests by 15 minutes,
so a steady 2 req/s stream reads `0.1 req/s`, and every reference screenshot
showed `0.0 req/s`. The M3 Mac checklist asks the user to look for a request
rate on each node, which would have looked broken.

## Decision

`rate_window_secs` in `crates/loctel-ui/src/app/data.rs` divides by the time
the store holds data for: from the oldest trace held to now, capped at the
range, at least one second. `Everything` has no cap. With no data the range is
used (the counts are zero anyway).

## Consequences

- Rates read true from the first minute.
- Once the data is older than the range, the rate is the range average, as
  before.
- The oldest trace held is used, not the oldest inside the range, so under a
  range the cap is what applies once the store holds older data. That is the
  intended reading: the rate over the range.
- `crates/loctel-ui/tests/live.rs` `map_rates_are_counts_over_the_range` pins
  both cases (young data, data older than the range), and `data.rs` has unit
  tests for the arithmetic.
