#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

if ! command -v cargo >/dev/null && [[ -f "$HOME/.cargo/env" ]]; then
    source "$HOME/.cargo/env"
fi

os=$(uname -s)
if [[ $os == Darwin ]]; then
    tools=(cargo rustc rustup cc clang git python3 xcrun)
else
    tools=(cargo rustc rustup cc clang cmake ninja pkg-config git python3)
fi
for tool in "${tools[@]}"; do
    command -v "$tool" >/dev/null || { echo "Missing tool: $tool" >&2; exit 1; }
done

uname -sm
rustc --version
cargo --version
rustup target list --installed

if [[ $os == Darwin ]]; then
    sw_vers
    # C/C++ dependencies (blake3, meshopt) compile against the macOS SDK.
    echo "macOS SDK: $(xcrun --show-sdk-path)"
    system_profiler SPDisplaysDataType | grep -E 'Chipset Model|Metal' || true
else
    pkg-config --modversion alsa libudev wayland-client xkbcommon x11 xcursor xi xrandr
    if command -v vulkaninfo >/dev/null; then
        vulkaninfo --summary
    else
        echo "vulkaninfo is unavailable; GPU detection was not checked."
    fi
fi

# Same lookup as struction_assets: $STRUCTION_BLENDER, else `blender` on PATH.
blender=${STRUCTION_BLENDER:-blender}
if command -v "$blender" >/dev/null; then
    "$blender" --background --factory-startup --python-expr \
        'import bpy; print("Blender ready:", bpy.app.version_string)'
else
    echo "Blender is unavailable; needed for .blend importing."
    if [[ $os == Darwin ]]; then
        echo "Set STRUCTION_BLENDER=/Applications/Blender.app/Contents/MacOS/Blender if it is installed there."
    fi
fi
