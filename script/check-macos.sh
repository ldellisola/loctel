#!/usr/bin/env bash
# Type-checks and lints the macOS code (loctel-macos and the macOS half of
# loctel-app) from Linux, for aarch64-apple-darwin. The Linux gate cannot see
# that code: it is compiled out there.
#
# Run it from anywhere: `bash script/check-macos.sh`. Extra arguments go to
# `cargo clippy` before the `--` (for example `--tests`).
#
# Why the wrapper: `cargo check` needs no linker, SDK or Apple tools, with one
# exception. gpui-pre-apple's build script compiles the Metal shaders only when
# it runs ON a Mac (its `#[cfg(target_os = "macos")]` tests the build machine,
# not the target), yet its source `include_bytes!`s the result on every
# machine. So when rustc compiles that one crate, this script first drops empty
# placeholder files where it will look. Nothing is linked or run, so empty
# files are enough. See docs/decisions/0009-macos-check-from-linux.md.
#
# Build scripts that compile C for the Mac (zstd-sys, under the store's segment
# files) cannot: a Linux machine has no Apple SDK headers, and its `cc` does not
# know `-arch`. So the C compiler for this one target is `script/stub-cc.sh`,
# which writes empty objects. Same reasoning: nothing is linked or run. See
# docs/decisions/0044-segment-file-format.md.
set -euo pipefail

if [ "${LOCTEL_RUSTC_WRAPPER:-}" = "1" ]; then
  # Called by cargo as `check-macos.sh <rustc> <args...>`.
  if [ "${CARGO_PKG_NAME:-}" = "gpui-pre-apple" ] && [ -n "${OUT_DIR:-}" ]; then
    : >>"$OUT_DIR/shaders.metallib"
    : >>"$OUT_DIR/stitched_shaders.metal"
  fi
  exec "$@"
fi

cd "$(dirname "${BASH_SOURCE[0]}")/.."
rustup target add aarch64-apple-darwin >/dev/null

export LOCTEL_RUSTC_WRAPPER=1
export RUSTC_WRAPPER="$PWD/script/check-macos.sh"
export CC_aarch64_apple_darwin="$PWD/script/stub-cc.sh"
exec cargo clippy --target aarch64-apple-darwin -p loctel-macos -p loctel-app "$@" -- -D warnings
