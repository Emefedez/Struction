# Live reload

Findings of the README's milestone 0 hotpatching spike, and how edits reach a running game.

## Recommendation

Use both, for different things:

- **Data reload is the live loop.** Everything the editor and AI tools author is JSONC data, so a running game should apply saved sources in place. This is implemented (`struction_world::live`) and works without extra tooling.
- **Rust hotpatching (`subsecond`) is a developer convenience, not a dependency.** It works with Bevy 0.19.1 and patches system bodies in about a second, but it needs the `dx` CLI and silently falls back to old code on edits it cannot patch. Keep it opt-in behind a `hotpatching` feature; nothing in the engine should require it.

The editor does not need a socket or a protocol to the running game: it already writes sources to disk through `AuthoringProject`, and the game polls the project. The same path covers AI tools and hand edits.

## Data reload

`LiveReloadPlugin` (add after `WorldPlugin`) scans the project every 250 ms and calls `reload_sources` in `WorldSet::Reload`, in `First`, so a change lands between fixed ticks. `reload_sources(world, files)` can also be called directly by a tool that knows what it wrote.

What it applies:

| Change | Effect on the running world |
| --- | --- |
| Definition or preset field | Rewritten on every live instance of it and its descendants, field by field. Fields the edit did not touch keep their runtime values: raising `Health.max` does not heal a wounded instance. |
| Component added or removed from a definition | Inserted or removed on live instances |
| Definition `transform` (scale) | Applied under the instance's runtime placement; a moved instance stays where it is |
| Spawn or spawner position, zone position | The instance is placed at the new authored position (the editor dragged it) |
| Spawn overrides | Applied like definition fields |
| Spawn removed, spawner removed | Its instances are despawned |
| New spawner | Spawns on the next tick |
| New spawn in a spawner that already ran | Not applied; reported in `LiveReloaded::restart_needed` |
| Broken source | Logged in `WorldErrors`; the last good definition stays live |

Instances record what they were built from in `Authored` only when `LiveReloadPlugin` is present, so shipped games pay nothing. The result of each reload is sent as a `LiveReloaded` message.

Try it: run `cargo run -p struction-playground -- --project examples/authoring` and, in another window, `cargo run -p struction-editor -- examples/authoring`. Instances show as orange blocks; moving the ogre or editing `guards/ogre` in the editor updates the playground. `apps/playground/src/authored.rs` includes the editor's `game.rs` so both apps register the same game; replace it with a game crate when one exists.

Verified headlessly: 4 `struction_world` tests (`tests/live.rs`) and a playground test that edits a copy of `examples/authoring` and sees the ogre move. Not yet run natively with a window.

Open items:

- Physics bodies: a re-placed instance gets a new `Transform`. Whether Avian carries that into `Position` for dynamic bodies is untested, because the authoring example has no bodies yet.
- Changes to `grantsToWards` still do not update existing wards (existing follow-up).
- Rename and file moves arrive as a delete plus a new file; references are fine after `rename_path`, but live instances of the old path are not re-pointed.

## Rust hotpatching

Bevy 0.19.1 ships `hotpatching` (subsecond 0.7 through `dioxus-devtools`). With it, every system body is called through subsecond's jump table and `DefaultPlugins` adds `HotPatchPlugin`. The apps expose it as a feature:

```sh
cargo install dioxus-cli --version 0.7.10 --locked   # about 12 minutes
dx serve --hot-patch -p struction-playground --features hotpatching
```

Measured on a headless Bevy 0.19.1 app in a scratch workspace (Linux, 4 cores):

| Case | Result |
| --- | --- |
| First `dx serve` build | 180 s from clean |
| Edit a system body in the binary crate | Patched in 0.3–1 s; `Local` state kept |
| Edit a system in a workspace library crate | Patched in 0.4 s. Struction's logic lives in `crates/`, so this matters |
| Change a system's parameters | Patch "succeeds", but the running app silently goes back to the originally compiled body. Restoring the signature recovers |
| Add a system in `main` | Ignored until restart: registration code already ran |
| Change a component's or resource's layout | Not tested; unsupported by subsecond and may crash |

Consequences for Struction:

- Good for tuning logic inside existing systems (controller, locomotion, AI conditions) without losing the scene.
- Anything that changes the shape of the app (new systems, new parameters, new components, plugin setup) needs a restart. The silent fallback is the main risk: the log says "Hot-patching … took" either way.
- It is a Linux/macOS/Windows native feature only, requires a matching `dx` version (`dioxus-cli` 0.7.x for Bevy 0.19), and doubles build configurations. Keep it out of CI.
- It does not replace data reload: authored content never needs recompiling.
