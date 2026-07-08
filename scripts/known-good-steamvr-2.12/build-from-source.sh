#!/usr/bin/env bash
# Rebuild stock ALVR v20.14.1 streamer (OpenVR 1.16.8) for SteamVR 2.12.14.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
INSTALL_DIR="${HOME}/.local/share/ALVR-Launcher/installations/v20.14.1/alvr_streamer_linux"
OPENVR_EXPECT="4c85abc"

cd "$REPO_ROOT"

echo "=== Build stock ALVR v20.14.1 ==="

git fetch --tags
git checkout v20.14.1
git submodule update --init openvr

OPENVR_SHA="$(git -C openvr rev-parse --short HEAD)"
if [[ "$OPENVR_SHA" != "$OPENVR_EXPECT" ]]; then
  echo "WARN: openvr at $OPENVR_SHA, expected $OPENVR_EXPECT (1.16.8)" >&2
fi

echo "Toolchain:"
rustc --version
cargo --version
nvcc --version 2>/dev/null | head -1 || echo "WARN: nvcc not found"

echo "Preparing dependencies (may take 30+ minutes on first run)..."
cargo xtask prepare-deps --platform linux

echo "Building release streamer..."
cargo xtask build-streamer --release

BUILD_OUT="$REPO_ROOT/build/alvr_streamer_linux"
if [[ ! -d "$BUILD_OUT" ]]; then
  echo "ERROR: build output not found at $BUILD_OUT" >&2
  exit 1
fi

echo "Verifying driver interface..."
if strings "$BUILD_OUT/lib64/alvr/bin/linux64/driver_alvr_server.so" 2>/dev/null | grep -q 'IVRDisplayComponent_003'; then
  echo "ERROR: built driver has IVRDisplayComponent_003 — wrong OpenVR version" >&2
  exit 1
fi
strings "$BUILD_OUT/lib64/alvr/bin/linux64/driver_alvr_server.so" | grep IVRDisplayComponent | head -3

read -r -p "Install to $INSTALL_DIR? [y/N] " ans
if [[ "${ans,,}" == "y" ]]; then
  mkdir -p "$(dirname "$INSTALL_DIR")"
  rm -rf "$INSTALL_DIR"
  cp -a "$BUILD_OUT" "$INSTALL_DIR"
  echo "Installed → $INSTALL_DIR"
  echo "Next: run restore.sh and fix vrcompositor wrapper (see README.md)"
else
  echo "Build artifacts left in: $BUILD_OUT"
fi
