"""Open an asset at a named workspace and part; do not save or alter source geometry."""
import bpy
import pathlib
import sys

source, workspace_name, object_name, material_name = sys.argv[sys.argv.index("--") + 1:]
source = pathlib.Path(source)
if source.suffix.lower() == ".blend":
    bpy.ops.wm.open_mainfile(filepath=str(source))
else:
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=str(source))


def focus():
    window = bpy.context.window
    if window is None:
        return 0.1
    workspace = bpy.data.workspaces.get(workspace_name)
    if workspace:
        window.workspace = workspace
    elif window.screen:
        area = next((area for area in window.screen.areas if area.type == "VIEW_3D"), None)
        if area:
            area.type = "IMAGE_EDITOR" if workspace_name == "UV Editing" else "NODE_EDITOR"
            if workspace_name != "UV Editing":
                area.ui_type = "ShaderNodeTree"
    if bpy.context.object and bpy.context.object.mode != "OBJECT":
        bpy.ops.object.mode_set(mode="OBJECT")
    objects = list(bpy.context.view_layer.objects)
    target = bpy.data.objects.get(object_name)
    if target is None and material_name:
        target = next((obj for obj in objects if obj.type == "MESH" and any(slot.material and slot.material.name == material_name for slot in obj.material_slots)), None)
    if target is None:
        target = next((obj for obj in objects if obj.type == "MESH"), None)
    if target:
        for obj in objects:
            obj.select_set(False)
        target.hide_set(False)
        target.select_set(True)
        bpy.context.view_layer.objects.active = target
        for index, slot in enumerate(target.material_slots):
            if slot.material and slot.material.name == material_name:
                target.active_material_index = index
                break
        if workspace_name == "UV Editing" and target.type == "MESH":
            bpy.ops.object.mode_set(mode="EDIT")
            bpy.ops.mesh.select_all(action="SELECT")
    return None


bpy.app.timers.register(focus, first_interval=0.2)
