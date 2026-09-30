# Engine toolbox plan

Prepared by **MOON**, 2026-09-30, from the README, design review, development,
authoring and debugging docs, and inspection of the current asset pipeline.
This is a work plan, not a claim that the tools below are implemented.

## Small programs and quick utilities

The toolbox launches lightweight, focused tools in either of two forms:

- **Mini-program:** a separate native window containing one task, its document
  or preview, a small toolset, undo and save/apply. Texture repair, behavior
  editing and pose editing fit this form. A floating panel inside the main
  editor is not the default interpretation of a separate window.
- **Quick utility:** click an action, choose a few settings or a preset, preview
  when useful, and apply or cancel. Collision generation, LOD generation and
  image resizing fit this form; they need no permanent workspace.

The portal is the launcher and optional **Open in…** handoff. Each mini-program
must offer a complete, straightforward task: useful defaults, a few meaningful
controls, feedback on the result and a way to revise or accept it. A button that
merely invokes another program does not provide that authoring experience.
Quick utilities can still be very small when a few choices genuinely complete
the task.

Choose the implementation around that experience. A small library, an adapted
open-source utility, a narrow custom toolset or a full application running in
the background can all be appropriate. Use a large application's capabilities
when they make the bounded task straightforward to deliver and use, without
making the user delve into that application's setup or concepts. Do not add a
large integration merely because it is available, or replace a working one
merely to make the backend smaller. Judge the whole workflow, including startup,
setup, responsiveness and maintenance.

There are two distinct ways to use a full application:

- **Internal backend:** for example, headless Blender supplies an operation
  behind the mini-program's own controls and preview. The user stays in the
  simple tool; Struction handles inputs, settings, results and errors.
- **Open in…:** the user deliberately opens the full application's interface
  when the task goes beyond the mini-program. This is optional escalation,
  separate from which backend implements the basic tool.

For UV preparation, the proposed workflow is to inspect the selected mesh with
a checker texture, keep its UVs or generate new ones, adjust only the necessary
packing controls, inspect the result, then revise or apply with undo. The
existing headless Blender operation is a reasonable starting backend for this
workflow. Consider another library only if a concrete limitation warrants it.
For a few texture brush strokes, a small raster canvas is likely the simpler
implementation; invoking a full painting application adds little to that task.

The windows can share the editor process and a serialized authoring host;
"mini-program" does not require a separate executable, plugin framework or a
second project writer. Open only the selected tool and its required resources.
Keep advanced settings out of the initial workflow and measure startup,
responsiveness and memory during implementation before setting numeric budgets.

Examples: **Fix texture** opens a small paint window with brush, eraser, color
picker and undo; **Generate collision** opens a shape/preset chooser; **Edit
behavior** opens a small behavior diagram with conditions and actions. Each
has an optional path to a suitable full application, including a code editor.

## Scope limits

This inventory is a map of possible entry points, not a commitment to build all
of them. Start with collision/LOD utilities and one mesh-inspection window.
Keep working hierarchy, inspector and play controls in the existing editor;
extract a mini-program only when a concrete authoring task benefits from it.

No vector editor, SVG pipeline, general image suite, full mesh modeler, new
visual scripting runtime or general plugin/window framework is planned.
Menus use existing controls and supported raster assets. Image conversion is
an option inside texture repair when needed, not another mini-program. Texture
repair starts with brush, eraser, picker, zoom, save and undo; defer extra tools
until a real task requires them. Painting and audio stay outside v1.

## Inventory and responsibilities

**MOON — next** marks my intended implementation work after this planning pass.
**SUN boundary** identifies existing ownership to coordinate with, not a new
assignment to SUN. **Unassigned** and **Later** are not active claims.
New libraries are candidates until checked against the pinned workspace versions.

| Tool / form | Small internal toolset | Base to reuse or evaluate | Optional larger tool | Work |
| --- | --- | --- | --- | --- |
| Toolbox launcher / quick actions | Open the selected asset's mini-program, reimport, show import errors and choose an external app | Existing asset/authoring APIs, egui, `OpenIn`, `SourceWatcher` | File-format-compatible app chosen by the user | **MOON — next** shared operations; window integration with SUN |
| Mesh inspector / window | Orbit model, inspect bounds/counts, check orientation and preview preparation results | Bevy mesh preview and existing loaders; headless Blender where its operations simplify a needed preparation task | Blender for modeling/sculpting | **MOON — next** operations; preview integration with SUN |
| UV preparation / utility with preview | Inspect checker result, preserve or generate UVs, adjust packing, revise and apply with undo | Existing headless **Blender Smart UV Project** as starting backend; **xatlas** only if a concrete workflow limitation warrants evaluation | Blender UV Editor for detailed seam/island authoring | **MOON — next** complete the bounded workflow |
| Collision generation / utility | Shape/preset selection, hull/decomposition controls, overlay and apply | Existing **Parry** hull/VHACD and **meshoptimizer**; add primitive fitting only where needed | Blender for hand-authored proxy geometry | **MOON — next** |
| LOD generation / utility | Quality preset, ratios/error limits, before/after preview and apply | Existing **meshoptimizer**, through `meshopt` | Blender for hand-crafted replacement meshes | **MOON — next** |
| Material editor / window | Texture slots, color, roughness/metalness and a small model preview | **Bevy StandardMaterial**, supported glTF material subset and egui controls | Blender for advanced material authoring/baking | Unassigned |
| Pose and target editor / window | Base poses, grips, seats, gaze targets and a few solver controls | Existing `struction_anim`, Bevy preview and egui | Blender for rigging/weight painting; code editor for solver implementation | **SUN boundary**, review/integration proposed |
| Procedural graph / window | Supported nodes, connections, curves and a live pose preview | Existing `GraphDef`; **egui-snarl candidate** for graph interaction | Code editor for custom Rust nodes and detailed data | Unassigned; coordinate with SUN |
| Surface and volume setup / utilities | Surface preset, friction, buoyancy, gravity or camera-zone settings, with contextual scene preview | Existing physics/gravity/character operations over **Avian/Bevy** | Code editor for behavior beyond the supplied components | **SUN boundary** for viewport; domain operations unassigned |
| Definition and encounter actions / existing editor or utility | Focused templates, overrides, spawn groups, master/ward links and reaction choices | Existing `AuthoringProject`, **jsonc-parser**, reflection/schema and world APIs | Code editor for JSONC/Rust | **MOON continuation** operations; SUN UI boundary; no duplicate editor |
| Behavior editor / window | Small diagram, condition/action selectors, sensing settings and running-state highlighting; state-machine interface is a candidate (see below) | Existing `struction_ai`; **egui-snarl candidate** canvas, engine metadata for choices | **Code editor** for complex behaviors and new Rust conditions/actions | Unassigned; SUN design review proposed |
| Play/trace inspection / existing editor | Select entity, inspect component changes, filter and step the isolated simulation | Existing `struction_debug`, JSONL and `AuthoringProject` | Code editor/debugger | **MOON continuation** operations; SUN UI boundary; separate trace window only if needed |
| Menu setup / existing editor initially | Arrange existing controls, labels, focus and action bindings using supported raster assets | **Bevy UI**, engine entity/action model and existing authoring controls | Code editor for behavior; texture-repair tool for raster art | Unassigned; no dedicated layout program yet |
| Texture repair / window | Brush, eraser, picker, zoom, save and undo on a flat RGBA image | **image** for image I/O; **imageproc candidate** only if drawing primitives are needed; a narrow paint canvas or adapted small open-source tool | **Krita or GIMP**, opening the same supported image | **Later**; MOON proposed for bounded implementation, not active |
| Audio trim / window | Waveform, preview, gain, trim and loop markers | Small waveform UI plus audio decoding/playback library to evaluate; **FFmpeg optional** for export/conversion | Audacity for recording, effects and multitrack edits | **Later**, unassigned |

## Behavior and painting boundaries

The behavior mini-program can be simple even when its advanced counterpart is
a code editor. A state-machine presentation (for example Idle, Patrol, Chase)
is a useful candidate. The adopted runtime currently uses behavior trees with
running status. Before implementing a state-machine authoring mode, define its
state persistence, entry/exit, transitions and execution mapping explicitly;
do not silently relabel tree nodes as states. A small tree editor is the direct
fit today. The user's example opens this design option without requiring a
runtime replacement in this documentation pass.

Texture repair should have its own small working toolset. Evaluate a compact
existing paint tool first; if adaptation costs more than the limited features,
build a narrow raster canvas on `image` and suitable drawing primitives.
The egui painting demo is an interaction reference, not a complete raster editor.
No particular third-party paint application has been selected. Layer stacks,
advanced brushes and painting directly on 3D models remain later extensions;
the first small repair tool edits a flat supported image. A small raster backend
is the current proposal because it fits these operations, not because full-app
backends are forbidden. Painting/audio remain later than the first engine version.

Reused or modified open-source code must keep required copyright and license
notices. Record upstream URL, version/commit, license and local modifications,
include required distribution notices, and credit the project in the tool's
About/Credits. Check the actual code's license before adapting it; a credit
alone does not grant reuse rights. Prefer small maintained components compatible
with the workspace over a new framework just to host one utility.

## What already exists and what is missing

The asset backend already imports `.blend`, `.gltf` and `.glb`, runs Blender to
generate missing UVs, generates collision and LODs, writes compiled mesh bundles,
loads them into Bevy, launches source applications and watches source saves.
The ownership/status table records MOON's earlier verification of 23 asset tests,
including real Blender import/export. This planning pass inspected code; it did
not repeat those runtime checks.

Those building blocks are not yet a complete authoring workflow:

- The editor still draws instance markers. Rendering imported meshes is an
  integration point with SUN's viewport work.
- The mesh bundle stores a material name, not a complete PBR material/texture
  representation. Full materials and texture dependencies need implementation.
- The compiled mesh path does not establish skeleton, skin or base-pose import.
  Specify those mappings before promising Blender-to-character round trips.
- `AuthoringProject` currently edits entity/preset/scene JSONC. Asset recipes,
  asset commands and their undo behavior need to join the shared authoring API.
- `OpenIn` defaults to Blender for `.blend` and `xdg-open` otherwise. Launching a
  program does not establish import support for its native format.
- Source watching exists, but tool integration must cover dependency changes,
  failed imports, last-good previews and conflicts with source/history edits.
- The animation runtime still uses a fixed solve pipeline rather than its graph.
  A node canvas alone would not make the procedural graph functional.

## MOON implementation order

1. **Prove a small utility:** collision and LOD operations using the existing
   libraries, with presets, preview data, revision, apply/cancel and recipe undo.
   Verify that users can assess and improve the result within the utility.
   Coordinate their small dialogs and preview hooks with SUN.
2. **Support mini-program windows:** supply shared document/revision/history
   operations; coordinate one separate mesh-inspector window with SUN. Keep one
   serialized authoring host and avoid duplicating the editor's mutation logic.
3. **Simple UV workflow:** build the checker-preview, adjust, regenerate and
   apply loop around the existing Blender operation. Expose only settings that
   help the task. Evaluate alternatives only if setup, responsiveness or result
   quality prevents this from being a straightforward utility.
4. **Optional external handoff:** choose an app compatible with the actual file,
   open it, and refresh supported saves/exports. Validate the existing Blender
   round trip as one case. Preserve last-good output on failed reimport.
5. **Authoring continuation:** rotation/scale and reference-safe rename/delete
   through the shared backend, coordinated with SUN's viewport needs.

Later, MOON is the proposed implementer for the bounded texture-repair tool.
This records a future direction without moving painting into v1.
SUN is the proposed reviewer for difficult multi-window/editor and animation or
behavior design decisions; this plan does not assign SUN new work or imply that
SUN has reviewed it.

Implementation changes to `apps/editor` or SUN's animation work require an
explicit ownership handoff. This plan claims the backend scope above, not those
files. Material, painting and audio tools remain separate work.

## Shared completion criteria

Every basic tool must offer inspection, validation and edits through structured
headless operations; GUI controls call those same operations. Source identity,
comments where applicable, revisions and undo remain intact. Save/import errors
identify the source and operation; missing external applications produce an
actionable diagnostic.

A mini-program opens in its own native window and closes independently of the
main editor. A quick utility presents only the choices needed for its operation.
The normal workflow completes inside the tool without requiring the user to
operate a full application's interface or configure its internals. A full app
may run behind it when that is the appropriate implementation. Verify the
complete select/edit/inspect/revise/apply workflow, including backend failures,
not just successful subprocess execution. Required backend dependencies must
have a straightforward setup path and clear missing-dependency errors. The
optional external-app button clearly names its destination. Closing with
unsaved work offers save/apply, discard or cancel, and external edits cannot
silently overwrite an open tool's working document.

Native sources remain editable in their original application. For formats such
as `.kra` and audio projects, the handoff must also define the export that the
engine consumes. External applications retain their own undo; Struction detects
outside changes and invalidates conflicting history rather than pretending to
undo another application's session. A round trip is complete only after the
supported export has been imported and validated in the engine.

## Upstream references

These establish upstream capabilities, not compatibility with Bevy 0.19.1.
Existing workspace dependencies stay pinned as documented.

- [image](https://github.com/image-rs/image): image buffers, format I/O and basic
  transforms; enable only needed format features to limit dependencies.
- [imageproc](https://github.com/image-rs/imageproc): candidate image processing
  and drawing primitives, not a complete paint application.
- [egui painting demo](https://github.com/emilk/egui/blob/main/crates/egui_demo_lib/src/demo/painting.rs):
  a small interaction reference to assess before reuse, with upstream credit.
- [xatlas](https://github.com/jpcy/xatlas): candidate mesh UV unwrapping/packing
  library, independent of a full modeling application.
- [Blender modeling, UV tools and painting](https://www.blender.org/features/modeling/)
  and [Blender features, including rigging](https://www.blender.org/features/).
- [meshoptimizer](https://github.com/zeux/meshoptimizer): mesh optimization and
  simplification, with the Rust `meshopt` binding.
- [Parry](https://parry.rs/): collision geometry foundation; the workspace's
  existing implementation uses hull generation and VHACD.
- [egui](https://github.com/emilk/egui) and
  [egui-snarl](https://github.com/zakarumych/egui-snarl): UI and candidate node
  canvas, respectively. The canvas supplies interaction, not runtime semantics.
- [Krita features](https://krita.org/en/features/): brushes, layers and seamless
  texture painting.
- [FFmpeg audio filters](https://ffmpeg.org/ffmpeg-filters.html) and
  [Audacity](https://www.audacityteam.org/): processing operations and the full
  audio editor.
