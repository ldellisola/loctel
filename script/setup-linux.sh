#!/usr/bin/env bash
# Linux development setup for loctel (Ubuntu 24.04, no GPU, no display).
#
# Idempotent: apt runs only when a package is missing, and rustup only
# installs what rust-toolchain.toml asks for and the machine lacks.
# Run it from anywhere: `bash script/setup-linux.sh`.
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

PACKAGES=(
  # toolchain basics
  build-essential pkg-config clang
  # windowing and fonts (GPUI)
  libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-xcb-dev
  libfontconfig-dev libfreetype-dev
  # software Vulkan/EGL so headless rendering finds an adapter (lavapipe)
  libvulkan1 mesa-vulkan-drivers libegl1 libegl-mesa0
  # virtual display and screenshot tooling, protobuf for OTLP
  xvfb xdotool imagemagick protobuf-compiler
)

# Run as root directly, otherwise through sudo.
if [ "$(id -u)" -eq 0 ]; then
  SUDO=()
else
  SUDO=(sudo)
fi

is_installed() {
  dpkg-query -W -f='${Status}' "$1" 2>/dev/null | grep -q '^install ok installed$'
}

missing=()
for pkg in "${PACKAGES[@]}"; do
  if ! is_installed "$pkg"; then
    missing+=("$pkg")
  fi
done

if [ "${#missing[@]}" -gt 0 ]; then
  echo "setup-linux: installing missing packages: ${missing[*]}"
  export DEBIAN_FRONTEND=noninteractive
  "${SUDO[@]}" apt-get update
  "${SUDO[@]}" apt-get install -y "${missing[@]}"
else
  echo "setup-linux: all apt packages already installed"
fi

# `rustup show` installs the toolchain pinned in rust-toolchain.toml
# (channel, components and targets) when it is not present yet.
rustup show
rustup target add aarch64-apple-darwin

echo "setup-linux: done"
