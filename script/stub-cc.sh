#!/usr/bin/env bash
# A stand-in C compiler that `script/check-macos.sh` hands to cc-rs for the
# aarch64-apple-darwin target. It "compiles" every file to an empty object and
# prints nothing, so a build script that compiles C sources (zstd-sys, for the
# store's segment files) finishes without Apple's SDK headers, which a Linux
# machine does not have. `cargo check` and `clippy` never link or run, so an
# empty library is enough. See docs/decisions/0044-segment-file-format.md.
#
# cc-rs calls it for a few probes (`-E`, `-?`, a flag check) that only look at
# the exit status, and then as `stub-cc.sh <flags> -o <object> -c <source>`.
# Because the stub prints nothing, cc-rs takes it for an MSVC-style compiler and
# says the object file as `-Fo<object>` instead, so both spellings are handled.
set -euo pipefail

previous=""
for argument in "$@"; do
  case "$argument" in
    -Fo?*) : >"${argument#-Fo}" ;;
    *)
      if [ "$previous" = "-o" ]; then
        : >"$argument"
      fi
      ;;
  esac
  previous="$argument"
done
