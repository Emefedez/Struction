# Development setup

The workspace contains an environment smoke test and a native physics playground using pinned Bevy 0.19.1. `rust-toolchain.toml` pins Rust 1.98.1, rustfmt, Clippy, rust-analyzer, Rust sources, and the WebAssembly target. Commit `Cargo.lock` when changing dependencies.

Two hosts are supported for development: Linux (the reference setup below, Omarchy on Apple Silicon) and [macOS](#macos). Everything after the host sections applies to both.

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

## macOS

Build natively for the host (`aarch64-apple-darwin` on Apple Silicon); the pinned toolchain installs it. Graphics use wgpu's Metal backend, so no Vulkan SDK or MoltenVK is needed. The applications' `wayland` and `x11` Bevy features only affect Linux and can stay enabled.

1. Install the Xcode Command Line Tools, which provide the linker, `clang`, and the macOS SDK that the C/C++ dependencies (`blake3`, `meshopt`) compile against:

   ```bash
   xcode-select --install
   ```

2. Install Rust with the [official rustup installer](https://rustup.rs/), then open a new terminal or run `source "$HOME/.cargo/env"`. The first `cargo` command in the repository downloads the pinned 1.98.1 toolchain.

3. Install [Blender](https://www.blender.org/download/) for `.blend` importing. The app bundle does not put `blender` on `PATH`; point Struction at it instead (for example in `~/.zshrc`):

   ```bash
   export STRUCTION_BLENDER=/Applications/Blender.app/Contents/MacOS/Blender
   ```

   `struction_assets` (importing, its Blender tests and **Open in…** for `.blend` files) reads `STRUCTION_BLENDER`, and so does `./tools/check-environment.sh`. Other files open with `open` on macOS and `xdg-open` on Linux.

Then follow [Native check](#native-check). No Linux packages, `pkg-config`, `cmake` or `ninja` are required.

Known gap on macOS: the `wasm-bindgen` version lookup under [WebAssembly](#webassembly) needs Python 3.11 or newer (`tomllib`). If the system `python3` is older, install a current Python, for example with Homebrew.

Verification status: the workspace type-checks for `aarch64-apple-darwin` from Linux (`cargo check --workspace --all-targets`, excluding `struction_assets`, whose `meshopt` C++ build needs the macOS SDK). Building, running and the Blender tests have not yet been verified on a Mac; record the first native run here.

## Native check

From the repository root:

```bash
./tools/check-environment.sh
cargo run --locked -p struction-smoke-test
```

The environment script checks the host's build dependencies, the GPU (Vulkan on Linux, Metal on macOS), and headless Blender. The application opens a lit rotating cube; Escape or closing the window exits. For an eight-second run:

```bash
cargo run --locked -p struction-smoke-test -- --smoke-test
```

On the Linux reference machine the renderer log identifies the Apple M1 Max and Honeykrisp/Vulkan; on macOS it should name the Apple GPU and the Metal backend. Initial compilation takes longer than subsequent builds.

## Playground

Run the physics scene with `cargo run -p struction-playground`. Use WASD to move, Space to jump or swim upward, and M to toggle mouse look and cursor grab. Escape releases a grabbed cursor; press it again to quit. The overlay shows contact, swimming, camera zone, footstep count, and FPS state. Walk over the blue slippery floor toward the gravity planet, or move right into the water pool.

Movement follows the camera's ground frame, including the overhead zone and changing gravity. The input adapter captures that frame in `CharacterIntent`; the fixed simulation never reads a camera. The player is a procedural `struction_anim` humanoid drawn with simple shapes: `CharacterAnimationPlugin` keeps its rig on the interpolated capsule and plants its feet with physics ray casts, on slopes, underwater floors and the planet alike. Ground and the water surface use a `StandardMaterial` extension (`sight_fade.wgsl`): when they block the camera's view of the player, a soft cylinder along the line of sight fades out, so the rest of the surface stays visible and a swimmer shows through the water. The water uses a two-sided surface at the fluid's upper boundary, separate from its buoyancy volume, to avoid blending overlapping box faces against the pool floor.

The scene is data. `apps/playground/project/` is a `struction_data` project: definitions such as `ground/slippery` (descending from `ground/stone`, which descends from the primordial `Ground`) and `scenes/milestone1.jsonc`, whose spawners place every floor, cube, volume, the planet and the player. `struction_world` spawns them in the first fixed tick. Only the camera, lights and HUD stay in code. Playground components carry the authoring side: `Shape` is a box or sphere that becomes the collider of anything with a `RigidBody` (and its mesh), `Look` gives the color and finish (`Matte`, `Ground` with the sight-line cut-out, or `Water`, a translucent surface on the top of a box `Volume`), and `Humanoid` attaches the animated rig. Engine components (`Surface`, `GravityField`, `Volume`, `CameraZone`, `CharacterController`, Avian's `RigidBody` and `ColliderDensity`) are written directly by their Rust names and fields. Edit the files and restart; `--project DIR` loads another copy. Data problems are logged with `file:line` at startup, and `cargo test -p struction-playground` fails on any of them.

`cargo run -p struction-playground -- --smoke-test` drives the character forward and jumps once, then logs its position and footstep count and exits after about ten seconds.

Headless world integration checks run with `cargo test -p struction_world --locked`. They load the fortress JSONC fixture, exercise boss/minion actions and adoption, restore saves, and rename paths while retaining comments and old save identities. Camera movement and overhead transitions have regression checks in `struction_character` and `struction-playground`; `cargo test -p struction_character --test animation` covers the rig following the body and foot placement on floors, slopes and in water.

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

On the Linux reference machine, native Vulkan execution and WebAssembly packaging passed. T3's embedded browser exposed WebGPU but returned no GPU adapter, so rendering there could not be validated. The web page checks adapter availability before starting.

## Other platforms

Linux, Windows, and macOS remain editor targets. macOS is covered [above](#macos). Windows builds need their own platform toolchain and validation; installing Rust targets on a Linux machine does not provide its SDK.

References: [Bevy setup](https://bevy.org/learn/quick-start/getting-started/setup/), [Linux dependencies](https://github.com/bevyengine/bevy/blob/v0.19.1/docs/linux_dependencies.md), [wgpu backends](https://github.com/gfx-rs/wgpu#supported-platforms), [Bevy web builds](https://github.com/bevyengine/bevy/blob/v0.19.1/examples/README.md#wasm).
