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

3. Install [Blender](https://www.blender.org/download/) for `.blend` importing. The app bundle does not put `blender` on `PATH`; `struction_assets` finds the newest `Blender*.app` in `/Applications` or `~/Applications` by itself (see [Blender bridge](#blender-bridge)). To use another one, set `STRUCTION_BLENDER` (for example in `~/.zshrc`):

   ```bash
   export STRUCTION_BLENDER=/Applications/Blender.app/Contents/MacOS/Blender
   ```

   `./tools/check-environment.sh` reads `STRUCTION_BLENDER` too.

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

## Build storage

Ordinary `cargo build`, `run`, `check` and `test` use compact workspace defaults: no debug symbols and no incremental compilation cache. Dependencies and executable outputs still live in `target/` and are reused; disabling incremental compilation trades slower rebuilds of changed crates for less disk usage. Debug assertions and overflow checks remain enabled. Avoid separate `CARGO_TARGET_DIR`s for routine verification, because they duplicate the dependency build.

Opt in to debugging artifacts when inspecting variables or source-level backtraces:

```bash
cargo run --profile diagnostic -p struction-playground
cargo test --profile diagnostic -p struction_character
```

This profile enables full debug symbols and incremental compilation and writes to `target/diagnostic/`. Cargo configuration or environment overrides (such as `CARGO_INCREMENTAL=1`) can override the compact defaults; use them deliberately.

Remove disposable build outputs and generated web bundles from this checkout with:

```bash
./tools/clean-artifacts.sh --dry-run
./tools/clean-artifacts.sh
```

Cleanup removes only this repository's `target/` and `dist/`, including diagnostic and old alternate build directories nested under `target/`. It preserves authored projects, Blender sources, compiled `.smesh` asset caches, installed dependencies and shared Cargo/Rust installations. Close running applications and finish builds before cleaning. The next Cargo invocation rebuilds what it needs. Cargo can retain obsolete outputs after feature/profile/compiler changes, so repeat cleanup when needed; compact defaults do not impose a disk quota.

Normal game/editor runs do not write trace or screenshot files. Request those explicitly with `--trace FILE.jsonl` or the editor's `--screenshot DIR`; generate a web bundle explicitly with `./tools/build-web.sh`.

## Playground

Run the physics scene with `cargo run -p struction-playground`. Use WASD to move, Space to jump or swim upward, Left Shift to [roll](moves.md), left click or F to [attack](moves.md), and M to toggle mouse look and cursor grab. V switches between third and first person; the mouse wheel zooms the third-person camera, and zooming in past its closest distance enters first person (zooming out leaves it). Escape releases a grabbed cursor; press it again to quit. The overlay shows contact, swimming, the running move, camera zone, footstep count, and FPS state. Walk over the blue slippery floor toward the gravity planet, or move right into the water pool; hit the orange cube or the knight standing guard to the left to knock them back.

Pass application options after Cargo's `--`, for example `cargo run -p struction-playground -- --project /path/to/project`. Run without `--project` for the bundled scene. The editor's `examples/authoring` project uses different game types and is not a playground scene.

Definition directories must not differ only in case: macOS usually treats those names as the same directory. The engine library uses `Terrain`, `terrains/`, `Region` and `regions/`; the playground's `aliases.jsonc` preserves former definition paths (`ground/`, `terrain/`, `gravity/`, `fields/scene`, `Ground`, `Gravity`) for saves. Author new sources and project overrides at the current engine paths, such as `terrains/planet`; aliases resolve saved identities, rather than redirecting source-file overrides. The playground's `authored_paths_fit_case_insensitive_filesystems` regression checks the project and engine content directories on Linux too.

The player camera (`struction_camera`) has only two views, never a free camera. In third person, mouse look orbits the camera around the player, the player turns toward where it walks, and a sphere cast pulls the camera in front of walls and ground behind it (water and camera zones do not). In first person the camera sits at the eyes and mouse look turns the player; the player's own rig is hidden. Switching views keeps the direction you were looking. Both views follow the player's local up, easing across gravity field changes, and the overhead camera zone reframes the third-person view only. Movement follows the camera's ground frame; the camera's input adapter captures that frame in `CharacterIntent`, and the fixed simulation never reads a camera. The player and the guard are procedural `struction_anim` humanoids dressed as the Blender-authored blood knight (see [Blender bridge](#blender-bridge)), or with simple shapes when no Blender or compiled model is available: `CharacterAnimationPlugin` keeps each rig on its interpolated capsule and plants its feet with physics ray casts, on slopes, underwater floors and the planet alike. Ground and the water surface use a `StandardMaterial` extension (`sight_fade.wgsl`): when they block the camera's view of the player, a soft cylinder along the line of sight fades out, so the rest of the surface stays visible and a swimmer shows through the water. The water uses a two-sided surface at the fluid's upper boundary, separate from its buoyancy volume, to avoid blending overlapping box faces against the pool floor.

The scene is data. `apps/playground/project/` holds the playground's scene, `scenes/milestone1.jsonc`, whose spawners place every floor, cube, volume, the planet, the player and the guard; `struction_world` spawns them in the first fixed tick. The definitions it places ship with the engine: `crates/struction_scene/content/definitions` is the read-only `engine` library every project descends from, with the primordials `Actor`, `Terrain` and `Region`, terrain (`terrains/slippery` descends from `terrains/stone`, which descends from `Terrain`), props (actors), regions (water, camera zones, `regions/scene_gravity`) and characters. A project file at an engine definition's path overrides it for every descendant (the editor creates one on the first edit); new definitions descend from the engine's. Only the camera, lights and HUD stay in code. `struction_scene` gives the authoring components effect: `Shape` is a box or sphere that becomes the collider of anything with a `RigidBody` (and its mesh), or `Rigged`, a named skeleton (`humanoid`) drawn as an animated rig with an optional `model`; `Look` gives the color and finish (`Matte`, `Ground` with the sight-line cut-out, or `Water`, a translucent surface on the top of a box `Volume`). Characters descend from `characters/humanoid`, an `Actor` with a controller and humanoid rig and nothing else, and name the [extensors](authoring-direction.md#extensors) they use: `characters/player` names `dodge` and `combat`, `characters/sentry` names none. Engine components (`Surface`, `GravityField`, `Volume`, `CameraZone`, `CharacterController`, Avian's `RigidBody` and `ColliderDensity`) are written directly by their Rust names and fields. Saved edits apply to the running playground (see [live reload](live-reload.md)); `--project DIR` loads another copy. Data problems are logged with `file:line` (`engine:` for library files) at startup, and `cargo test -p struction-playground` fails on any of them.

`cargo run -p struction-playground -- --smoke-test` drives the character forward and jumps once, then logs its position and footstep count and exits after about ten seconds.

Headless world integration checks run with `cargo test -p struction_world --locked`. They load the fortress JSONC fixture, exercise boss/minion actions and adoption, restore saves, and rename paths while retaining comments and old save identities. Camera views, zoom, collision, planets and overhead transitions have regression checks in `cargo test -p struction_camera`, and camera-relative movement in `struction_character`; `cargo test -p struction_character --test animation` covers the rig following the body and foot placement on floors, slopes and in water.

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

The engine's knight model, `crates/struction_scene/content/assets/models/blood_knight.blend`, is generated by `blood_knight.py` next to it and committed so it opens in Blender directly. Each object is a rigid armor piece named `<joint>.<piece>` after the `struction_anim` humanoid joint it rides on, with its origin at that joint's rest position (Blender Z up, facing +Y, arms hanging, soles on Z = 0). Any humanoid rig can wear it, or another model built the same way: `"Shape": { "Rigged": { "rig": "humanoid", "model": "engine://models/blood_knight.blend" } }`. `engine://` is the engine's asset source (`EngineAssetsPlugin`, added before `DefaultPlugins`); paths without it are the app's own `assets/`. The first time an entity uses a model, the app compiles it on a background thread when it changed (the `.smesh` is ignored by git), and while it runs it watches the file: edit the knight in Blender, save, and the running playground re-dresses every actor wearing it. Regenerate from the script with:

```bash
cargo run -p struction_assets --bin struction-assets -- build \
  crates/struction_scene/content/assets/models/blood_knight.py \
  crates/struction_scene/content/assets/models/blood_knight.blend
```

Saving over the generated file discards manual edits, so choose one: edit the script and regenerate, or keep editing the `.blend`. A `.blend` saved by a newer Blender may not open in an older one; it was generated with Blender 5.0. `cargo test -p struction-playground` checks that every piece sits on a humanoid joint (skipped without Blender).

## VS Code extension

`extensions/vscode` is a Node extension (no bundler, `tsc` only) over the read-only language host. Build the host and put it on `PATH`, or point the extension at it:

```bash
cargo install --path crates/struction_language --locked
cd extensions/vscode && npm install && npm run compile
```

Open `extensions/vscode` in VS Code and press F5 ("Struction Extension") to launch an extension host against this workspace. Without a workspace setting the extension discovers `project/` or `apps/playground/project/`, looks for `struction-language` on `PATH`, and restarts when `struction.projectRoot`, `struction.hostPath` or `struction.hostArgs` changes. The status bar shows whether a project was found, whether a snapshot is being computed, and how many problems the host reported; the **Struction** output channel carries the host's stderr and any diagnostics without a file.

Tests are headless: `npm test` compiles and runs `test/*.test.ts` against the feature layer, which imports no `vscode` module, and `cargo test -p struction_language` covers the snapshots and the JSONL transport. Neither needs VS Code or a running host.

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
