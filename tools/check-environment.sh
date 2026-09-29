#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

if ! command -v cargo >/dev/null && [[ -f "$HOME/.cargo/env" ]]; then
    source "$HOME/.cargo/env"
fi

for tool in cargo rustc rustup cc clang cmake ninja pkg-config git python3; do
    command -v "$tool" >/dev/null || { echo "Missing tool: $tool" >&2; exit 1; }
done

uname -sm
rustc --version
cargo --version
rustup target list --installed
pkg-config --modversion alsa libudev wayland-client xkbcommon x11 xcursor xi xrandr

if command -v vulkaninfo >/dev/null; then
    vulkaninfo --summary
else
    echo "vulkaninfo is unavailable; GPU detection was not checked."
fi

if command -v blender >/dev/null; then
    blender --background --factory-startup --python-expr \
        'import bpy; print("Blender ready:", bpy.app.version_string)'
else
    echo "Blender is unavailable; needed later for .blend importing."
fi
