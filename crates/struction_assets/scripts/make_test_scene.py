"""Build the known test scene used by struction_assets tests.

    blender --background --factory-startup --python-exit-code 1 \
        --python make_test_scene.py -- <output.blend>

Objects (Blender Z-up coordinates, meters):
- "Tower": box 2 x 1 x 4 (X x Y x Z) at (1, 2, 3), material "Stone", no UV map.
- "Ball": smooth UV sphere of radius 1 (32 segments, 16 rings) at (-3, 0, 1),
  material "Metal", with a UV map.
"""

import sys

import bmesh
import bpy


def material(name):
    return bpy.data.materials.get(name) or bpy.data.materials.new(name)


def add_object(name, mesh, location, material_name):
    mesh.materials.append(material(material_name))
    obj = bpy.data.objects.new(name, mesh)
    obj.location = location
    bpy.context.scene.collection.objects.link(obj)
    return obj


def tower():
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0, calc_uvs=False)
    bmesh.ops.scale(bm, vec=(2.0, 1.0, 4.0), verts=bm.verts)
    mesh = bpy.data.meshes.new("Tower")
    bm.to_mesh(mesh)
    bm.free()
    assert len(mesh.uv_layers) == 0
    return add_object("Tower", mesh, (1.0, 2.0, 3.0), "Stone")


def ball():
    bm = bmesh.new()
    bm.loops.layers.uv.new("UVMap")
    bmesh.ops.create_uvsphere(bm, u_segments=32, v_segments=16, radius=1.0, calc_uvs=True)
    mesh = bpy.data.meshes.new("Ball")
    bm.to_mesh(mesh)
    bm.free()
    for polygon in mesh.polygons:
        polygon.use_smooth = True
    return add_object("Ball", mesh, (-3.0, 0.0, 1.0), "Metal")


def main():
    output = sys.argv[sys.argv.index("--") + 1]
    bpy.ops.wm.read_factory_settings(use_empty=True)
    tower()
    ball()
    bpy.ops.wm.save_as_mainfile(filepath=output)


main()
