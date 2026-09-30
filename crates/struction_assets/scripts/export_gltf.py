"""Convert a .blend or glTF source to GLB for the Struction asset pipeline.

Run by struction_assets through headless Blender:

    blender --background --factory-startup --disable-autoexec --python-exit-code 1 \
        --python export_gltf.py -- <input> <output.glb> [--generate-uvs]

Axis conversion is left to the glTF exporter (+Y up): Blender (x, y, z) becomes
engine (x, z, -y), in meters. With --generate-uvs, meshes without a UV map get
one from Smart UV Project before exporting.
"""

import math
import sys

import bpy


def parse_args():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    flags = {arg for arg in argv if arg.startswith("--")}
    paths = [arg for arg in argv if not arg.startswith("--")]
    unknown = flags - {"--generate-uvs"}
    if len(paths) != 2 or unknown:
        raise SystemExit(
            "usage: export_gltf.py -- <input> <output.glb> [--generate-uvs]"
        )
    return paths[0], paths[1], "--generate-uvs" in flags


def load(source):
    if source.lower().endswith(".blend"):
        bpy.ops.wm.open_mainfile(filepath=source, load_ui=False)
    else:
        bpy.ops.wm.read_factory_settings(use_empty=True)
        bpy.ops.import_scene.gltf(filepath=source)


def generate_missing_uvs():
    view_layer = bpy.context.view_layer
    targets = {}
    for obj in view_layer.objects:
        if obj.type == "MESH" and len(obj.data.uv_layers) == 0 and obj.visible_get():
            # Linked duplicates share their mesh; unwrap it once.
            targets.setdefault(obj.data.name, obj)
    if not targets:
        return

    if bpy.context.object is not None and bpy.context.object.mode != "OBJECT":
        bpy.ops.object.mode_set(mode="OBJECT")
    bpy.ops.object.select_all(action="DESELECT")
    for obj in targets.values():
        obj.data.uv_layers.new(name="UVMap")
        obj.select_set(True)
        view_layer.objects.active = obj
    bpy.ops.object.mode_set(mode="EDIT")
    bpy.ops.mesh.select_all(action="SELECT")
    bpy.ops.uv.smart_project(angle_limit=math.radians(66.0), island_margin=0.02)
    bpy.ops.object.mode_set(mode="OBJECT")
    for name in sorted(targets):
        print(f"STRUCTION: generated UVs for mesh {name!r}")


def export(output):
    bpy.ops.export_scene.gltf(
        filepath=output,
        export_format="GLB",
        export_yup=True,
        export_apply=True,
        export_texcoords=True,
        export_normals=True,
        export_tangents=False,
        export_materials="EXPORT",
        export_image_format="NONE",
        export_animations=False,
        export_skins=False,
        export_morph=False,
        export_cameras=False,
        export_lights=False,
        export_extras=False,
        use_selection=False,
        use_visible=False,
    )


def main():
    source, output, generate_uvs = parse_args()
    load(source)
    if generate_uvs:
        generate_missing_uvs()
    export(output)


main()
