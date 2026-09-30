# Development setup

The workspace contains an environment smoke test and a native physics playground using pinned Bevy 0.19.1. `rust-toolchain.toml` pins Rust 1.98.1, rustfmt, Clippy, rust-analyzer, Rust sources, and the WebAssembly target. Commit `Cargo.lock` when changing dependencies.

## Omarchy on Apple Silicon

Build natively for `aarch64-unknown-linux-gnu`. Graphics use Asahi's Mesa/Honeykrisp Vulkan driver and the Wayland desktop; x86 emulation is unnecessary.

The required packages on this machine were already installed:

```bash
omarchy pkg add base-devel clang cmake ninja pkgconf git \
  alsa-lib systemd-libs wayland libxkbcommon libx11 libxcursor libxi libxrandr \
  mesa vulkan-asahi vulkan-icd-loader vulkan-tools
```

These package names are for Arch Linux ARM with Apple Silicon. Use the appropriate GPU driver on other hardware. The existing native ARM64 Blender installation is available as `blender`.

Install Rust through the [official rustup installer](https://rustup.rs/) if absent. On this machine it is installed under `~/.cargo` and `~/.rustup`. New login shells load it automatically; in an existing terminal run:

```bash
source "$HOME/.cargo/env"
```

The pinned toolchain includes Cargo, rustfmt, Clippy, rust-analyzer, and sources for editor navigation. C/C++ build tools and GDB are also available. No custom linker configuration is required.

## Native check

From the repository root:

```bash
./tools/check-environment.sh
cargo run --locked -p struction-smoke-test
```

The environment script checks Linux build dependencies, Vulkan, and headless Blender. The application opens a lit rotating cube; Escape or closing the window exits. For an eight-second run:

```bash
cargo run --locked -p struction-smoke-test -- --smoke-test
```

The renderer log should identify the Apple M1 Max and Honeykrisp/Vulkan. Initial compilation takes longer than subsequent builds.

## Playground

Run the physics scene with `cargo run -p struction-playground`. Use WASD to move, Space to jump or swim upward, and M to toggle mouse look and cursor grab. Escape releases a grabbed cursor; press it again to quit. The overlay shows contact, swimming, camera zone, footstep count, and FPS state. Walk over the blue slippery floor toward the gravity planet, or move right into the water pool.

Movement follows the camera's ground frame, including the overhead zone and changing gravity. The input adapter captures that frame in `CharacterIntent`; the fixed simulation never reads a camera. The player is a procedural `struction_anim` humanoid dressed as the Blender-authored blood knight (see [Blender bridge](#blender-bridge)), or with simple shapes when no Blender or compiled model is available: `CharacterAnimationPlugin` keeps its rig on the interpolated capsule and plants its feet with physics ray casts, on slopes, underwater floors and the planet alike. Ground and the water surface use a `StandardMaterial` extension (`sight_fade.wgsl`): when they block the camera's view of the player, a soft cylinder along the line of sight fades out, so the rest of the surface stays visible and a swimmer shows through the water. The water uses a two-sided surface at the fluid's upper boundary, separate from its buoyancy volume, to avoid blending overlapping box faces against the pool floor.

`cargo run -p struction-playground -- --smoke-test` drives the character forward and jumps once, then logs its position and footstep count and exits after about ten seconds.

Headless world integration checks run with `cargo test -p struction_world --locked`. They load the fortress JSONC fixture, exercise boss/minion actions and adoption, restore saves, and rename paths while retaining comments and old save identities. Camera movement and overhead transitions have regression checks in `struction_character` and `struction-playground`; `cargo test -p struction_character --test animation` covers the rig following the body and foot placement on floors, slopes and in water.

For structured status and position changes, add `--trace /tmp/struction-trace.jsonl` (a new file). This can be combined with `--smoke-test`. See [debugging](debugging.md) for the event format, filtering examples, and headless trace API.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Blender bridge

`struction_assets` runs Blender headless as a subprocess; games load the compiled `.smesh` and never need Blender. It uses `$STRUCTION_BLENDER` if set, else `blender` on `PATH`, else on macOS the newest `Blender*.app` in `/Applications` or `~/Applications` (`/Applications/Blender.app/Contents/MacOS/Blender`). "Open in…" opens `.blend` files in that same Blender, and other files with `open` on macOS or `xdg-open` on Linux.

- `struction-assets build <generator.py> <out.blend|.glb>` runs a Blender Python script that builds an asset from code (it receives the output path after `--`).
- `struction-assets compile <source>` converts `.blend`/glTF to `.smesh`: geometry, UVs, LODs, collision, the node hierarchy and each material's base color, metallic, roughness and emission (constant factors; textures are not exported yet).
- `struction-assets inspect <file.smesh>` lists what was compiled.

Run them with `cargo run -p struction_assets --bin struction-assets -- <command> …`.

The playground's player model, `apps/playground/assets/models/blood_knight.blend`, is generated by `blood_knight.py` next to it and committed so it opens in Blender directly. Each object is a rigid armor piece named `<joint>.<piece>` after the `struction_anim` humanoid joint it rides on, with its origin at that joint's rest position (Blender Z up, facing +Y, arms hanging, soles on Z = 0). On start the playground compiles the `.blend` when it changed (the `.smesh` is ignored by git), and while it runs it watches the file: edit the knight in Blender, save, and the running playground re-dresses the player. Regenerate from the script with:

```bash
cargo run -p struction_assets --bin struction-assets -- build \
  apps/playground/assets/models/blood_knight.py apps/playground/assets/models/blood_knight.blend
```

Saving over the generated file discards manual edits, so choose one: edit the script and regenerate, or keep editing the `.blend`. A `.blend` saved by a newer Blender may not open in an older one; it was generated with Blender 5.0. `cargo test -p struction-playground` checks that every piece sits on a humanoid joint (skipped without Blender).

## WebAssembly

The pinned toolchain installs `wasm32-unknown-unknown`. The `wasm-bindgen` CLI version must match the `wasm-bindgen` crate in `Cargo.lock`. Install the matching version with:

```bash
version=$(python3 -c 'import tomllib; d=tomllib.load(open("Cargo.lock", "rb")); print(next(p["version"] for p in d["package"] if p["name"] == "wasm-bindgen"))')
cargo install wasm-bindgen-cli --version "$version" --locked
./tools/build-web.sh
python3 -m http.server 8000 --bind 127.0.0.1 --directory dist/smoke-test
```

Open `http://localhost:8000` in a browser with WebGPU enabled and supported. Compilation alone does not establish browser GPU support. The generated files in `dist/` are disposable.

On this machine, native Vulkan execution and WebAssembly packaging passed. T3's embedded browser exposed WebGPU but returned no GPU adapter, so rendering there could not be validated. The web page checks adapter availability before starting.

## Other platforms

Linux, Windows, and macOS remain editor targets. Windows and macOS builds need their own platform toolchains and validation; installing Rust targets on this Linux machine does not provide those SDKs.

References: [Bevy setup](https://bevy.org/learn/quick-start/getting-started/setup/), [Linux dependencies](https://github.com/bevyengine/bevy/blob/v0.19.1/docs/linux_dependencies.md), [Bevy web builds](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/README.md#wasm).
