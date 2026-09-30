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

Run the physics scene with `cargo run -p struction-playground`. Use WASD to move, Space to jump or swim upward, and M to toggle mouse look and cursor grab. V switches between third and first person; the mouse wheel zooms the third-person camera, and zooming in past its closest distance enters first person (zooming out leaves it). Escape releases a grabbed cursor; press it again to quit. The overlay shows contact, swimming, camera zone, footstep count, and FPS state. Walk over the blue slippery floor toward the gravity planet, or move right into the water pool.

The player camera (`struction_camera`) has only two views, never a free camera. In third person, mouse look orbits the camera around the player, the player turns toward where it walks, and a sphere cast pulls the camera in front of walls and ground behind it (water and camera zones do not). In first person the camera sits at the eyes and mouse look turns the player; the player's own rig is hidden. Switching views keeps the direction you were looking. Both views follow the player's local up, easing across gravity field changes, and the overhead camera zone reframes the third-person view only. Movement follows the camera's ground frame; the camera's input adapter captures that frame in `CharacterIntent`, and the fixed simulation never reads a camera. The player is a procedural `struction_anim` humanoid drawn with simple shapes: `CharacterAnimationPlugin` keeps its rig on the interpolated capsule and plants its feet with physics ray casts, on slopes, underwater floors and the planet alike. Ground and the water surface use a `StandardMaterial` extension (`sight_fade.wgsl`): when they block the camera's view of the player, a soft cylinder along the line of sight fades out, so the rest of the surface stays visible and a swimmer shows through the water. The water uses a two-sided surface at the fluid's upper boundary, separate from its buoyancy volume, to avoid blending overlapping box faces against the pool floor.

`cargo run -p struction-playground -- --smoke-test` drives the character forward and jumps once, then logs its position and footstep count and exits after about ten seconds.

Headless world integration checks run with `cargo test -p struction_world --locked`. They load the fortress JSONC fixture, exercise boss/minion actions and adoption, restore saves, and rename paths while retaining comments and old save identities. Camera views, zoom, collision, planets and overhead transitions have regression checks in `cargo test -p struction_camera`, and camera-relative movement in `struction_character`; `cargo test -p struction_character --test animation` covers the rig following the body and foot placement on floors, slopes and in water.

For structured status and position changes, add `--trace /tmp/struction-trace.jsonl` (a new file). This can be combined with `--smoke-test`. See [debugging](debugging.md) for the event format, filtering examples, and headless trace API.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

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
