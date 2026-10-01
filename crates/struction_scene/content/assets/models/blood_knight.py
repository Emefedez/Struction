"""The blood knight: the playground's player model, built from code in Blender.

    cargo run -p struction_assets --bin struction-assets -- \
        build apps/playground/assets/models/blood_knight.py \
        apps/playground/assets/models/blood_knight.blend

or, with Blender itself (macOS: /Applications/Blender.app/Contents/MacOS/Blender):

    blender --background --factory-startup --python blood_knight.py -- blood_knight.blend

A crimson plate knight on the heavy side, with a katana in his right hand, an
empty scabbard on his left hip and a gold-ringed blood drop on a black field
across his back. The saved .blend is an ordinary Blender file: open it, edit
it, save it, and the running playground picks the change up.

Rig binding: the armor is rigid pieces for struction_anim's built-in humanoid,
no skinning. Every object is named after the rig joint it rides on,
`<joint>.<piece>`, and its origin is that joint's rest position, so the engine
parents it to the joint without offsets. Blender Z up, meters; the knight faces
+Y (the engine's -Z forward) and his right hand is +X. Rest pose: arms hanging,
legs straight, soles on Z = 0.
"""

import math
import sys

import bmesh
import bpy
from mathutils import Matrix, Vector

# Rest positions of struction_anim::humanoid::rig(), in Blender axes.
JOINTS = {
    "hips": (0.0, 0.0, 1.0),
    "spine": (0.0, 0.0, 1.12),
    "chest": (0.0, 0.0, 1.30),
    "neck": (0.0, 0.0, 1.52),
    "head": (0.0, 0.0, 1.62),
}
for _suffix, _side in (("l", -1.0), ("r", 1.0)):
    JOINTS[f"upper_arm_{_suffix}"] = (_side * 0.19, 0.0, 1.46)
    JOINTS[f"forearm_{_suffix}"] = (_side * 0.19, 0.0, 1.18)
    JOINTS[f"hand_{_suffix}"] = (_side * 0.19, 0.0, 0.92)
    JOINTS[f"thigh_{_suffix}"] = (_side * 0.09, 0.0, 0.96)
    JOINTS[f"shin_{_suffix}"] = (_side * 0.09, 0.0, 0.50)
    JOINTS[f"foot_{_suffix}"] = (_side * 0.09, 0.0, 0.08)


def principled(name, color, metallic, roughness, emission=None, strength=0.0):
    material = bpy.data.materials.new(name)
    material.use_nodes = True
    bsdf = material.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (*color, 1.0)
    bsdf.inputs["Metallic"].default_value = metallic
    bsdf.inputs["Roughness"].default_value = roughness
    if emission is not None:
        bsdf.inputs["Emission Color"].default_value = (*emission, 1.0)
        bsdf.inputs["Emission Strength"].default_value = strength
    material.diffuse_color = (*color, 1.0)
    return material


def materials():
    return {
        "crimson": principled("Crimson Plate", (0.42, 0.015, 0.02), 0.8, 0.3),
        "black": principled("Blackened Steel", (0.025, 0.022, 0.026), 0.9, 0.42),
        "gold": principled("Gold Trim", (0.85, 0.52, 0.14), 1.0, 0.28),
        "blade": principled("Blade Steel", (0.92, 0.93, 0.95), 0.6, 0.22),
        "wrap": principled("Black Wrap", (0.018, 0.012, 0.012), 0.0, 0.85),
        "leather": principled("Dark Leather", (0.03, 0.011, 0.006), 0.0, 0.7),
        "glow": principled(
            "Blood Glow", (0.25, 0.0, 0.005), 0.0, 0.45, emission=(1.0, 0.02, 0.01), strength=1.0
        ),
    }


# Primitives, built in world coordinates. Each returns its new vertices.


def created(result):
    return list(result["verts"])


def smooth(bm, verts):
    for face in {face for vert in verts for face in vert.link_faces}:
        face.smooth = True


def box(bm, center, size, bevel=0.0, rotation=None):
    verts = created(bmesh.ops.create_cube(bm, size=1.0))
    matrix = Matrix.Translation(center) @ (rotation or Matrix()) @ Matrix.Diagonal((*size, 1.0))
    bmesh.ops.transform(bm, matrix=matrix, verts=verts)
    if bevel > 0.0:
        edges = list({edge for vert in verts for edge in vert.link_edges})
        result = bmesh.ops.bevel(
            bm, geom=verts + edges, offset=bevel, segments=2, affect="EDGES", profile=0.5
        )
        verts = [v for v in verts if v.is_valid] + list(result["verts"])
    return verts


def ellipsoid(bm, center, radii, segments=20, rings=12, rotation=None):
    verts = created(
        bmesh.ops.create_uvsphere(bm, u_segments=segments, v_segments=rings, radius=1.0)
    )
    matrix = Matrix.Translation(center) @ (rotation or Matrix()) @ Matrix.Diagonal((*radii, 1.0))
    bmesh.ops.transform(bm, matrix=matrix, verts=verts)
    smooth(bm, verts)
    return verts


def loft(bm, rings, segments=24, cx=0.0):
    """A closed, smooth vertical shape through elliptical rings
    `(z, rx, ry, cy)`, bottom to top: torsos, helms and shaped limbs."""
    loops = []
    for z, rx, ry, cy in rings:
        loops.append(
            [
                bm.verts.new(
                    (
                        cx + rx * math.cos(2 * math.pi * i / segments),
                        cy + ry * math.sin(2 * math.pi * i / segments),
                        z,
                    )
                )
                for i in range(segments)
            ]
        )
    for a, b in zip(loops, loops[1:]):
        for i in range(segments):
            j = (i + 1) % segments
            bm.faces.new((a[i], a[j], b[j], b[i]))
    for loop, (z, _, _, cy), reverse in ((loops[0], rings[0], True), (loops[-1], rings[-1], False)):
        cap = bm.verts.new((cx, cy, z))
        for i in range(segments):
            j = (i + 1) % segments
            bm.faces.new((loop[j], loop[i], cap) if reverse else (loop[i], loop[j], cap))
    verts = [v for loop in loops for v in loop]
    smooth(bm, verts)
    return verts


def limb(bm, x, profile, cy=0.0):
    """A round limb along Z at `x`; `profile` is (z, radius) pairs, top to bottom."""
    rings = [(z, r, r * 0.95, cy) for z, r in reversed(profile)]
    return loft(bm, rings, segments=18, cx=x)


def tube(bm, start, end, radius_start, radius_end, segments=18):
    """A capped, possibly tapered cylinder from `start` to `end`."""
    start, end = Vector(start), Vector(end)
    axis = end - start
    verts = created(
        bmesh.ops.create_cone(
            bm,
            cap_ends=True,
            segments=segments,
            radius1=radius_start,
            radius2=radius_end,
            depth=axis.length,
        )
    )
    rotation = Vector((0.0, 0.0, 1.0)).rotation_difference(axis).to_matrix().to_4x4()
    bmesh.ops.transform(bm, matrix=Matrix.Translation((start + end) / 2) @ rotation, verts=verts)
    smooth(bm, verts)
    return verts


def torus(bm, center, normal, major, minor, segments=32, sides=10):
    rotation = Vector((0.0, 0.0, 1.0)).rotation_difference(Vector(normal)).to_matrix()
    rings = []
    for i in range(segments):
        a = 2 * math.pi * i / segments
        ring = []
        for j in range(sides):
            b = 2 * math.pi * j / sides
            r = major + minor * math.cos(b)
            local = Vector((r * math.cos(a), r * math.sin(a), minor * math.sin(b)))
            ring.append(bm.verts.new(Vector(center) + rotation @ local))
        rings.append(ring)
    for i in range(segments):
        for j in range(sides):
            a, b = rings[i], rings[(i + 1) % segments]
            bm.faces.new((a[j], b[j], b[(j + 1) % sides], a[(j + 1) % sides]))
    verts = [v for ring in rings for v in ring]
    smooth(bm, verts)
    return verts


def prism(bm, outline, origin, u, v, depth):
    """Extrudes a 2D outline (in the plane spanned by u, v) by `depth` along u x v."""
    origin, u, v = Vector(origin), Vector(u), Vector(v)
    normal = u.cross(v).normalized()
    front = [bm.verts.new(origin + u * x + v * y + normal * depth / 2) for x, y in outline]
    back = [bm.verts.new(origin + u * x + v * y - normal * depth / 2) for x, y in outline]
    bm.faces.new(front)
    bm.faces.new(list(reversed(back)))
    n = len(outline)
    for i in range(n):
        bm.faces.new((back[i], back[(i + 1) % n], front[(i + 1) % n], front[i]))
    return front + back


def blood_drop(points=24):
    """Outline of a drop, point up: the tip, then around a circle between the
    two tangent points."""
    radius, tip = 0.042, 0.1
    theta = math.acos(radius / tip)
    start = math.pi / 2 + theta
    sweep = 2 * math.pi - 2 * theta
    outline = [(0.0, tip)]
    for i in range(points + 1):
        a = start + sweep * i / points
        outline.append((radius * math.cos(a), radius * math.sin(a)))
    return outline


def katana_blade(bm, base, direction, up, length=0.72, width=0.032, thickness=0.008, sori=0.03):
    """A curved, single-edged blade: spine toward `up`, edge away from it, the
    kissaki a single point near the spine."""
    base, direction, up = Vector(base), Vector(direction), Vector(up)
    side = direction.cross(up).normalized()
    sections = 16
    body = length * 0.93

    def center(distance):
        s = distance / length
        return base + direction * distance + up * (sori * s * s)

    rows = []
    for i in range(sections + 1):
        distance = body * i / sections
        w = width * (1.0 - 0.2 * distance / length)
        t = thickness * (1.0 - 0.4 * distance / length)
        c = center(distance)
        rows.append(
            [
                bm.verts.new(c + up * (w / 2) + side * (t / 2)),
                bm.verts.new(c + up * (w / 2) - side * (t / 2)),
                bm.verts.new(c - up * (w / 2)),
            ]
        )
    for a, b in zip(rows, rows[1:]):
        for j in range(3):
            k = (j + 1) % 3
            bm.faces.new((a[j], b[j], b[k], a[k]))
    tip = bm.verts.new(center(length) + up * (width * 0.3))
    last = rows[-1]
    for j in range(3):
        bm.faces.new((last[j], tip, last[(j + 1) % 3]))
    bm.faces.new(list(reversed(rows[0])))
    return [v for row in rows for v in row] + [tip]


# Assembly.


def add_part(joint, piece, material, build):
    bm = bmesh.new()
    build(bm)
    offset = Vector(JOINTS[joint])
    bmesh.ops.translate(bm, vec=-offset, verts=bm.verts)
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    mesh = bpy.data.meshes.new(f"{joint}.{piece}")
    bm.to_mesh(mesh)
    bm.free()
    mesh.materials.append(material)
    obj = bpy.data.objects.new(f"{joint}.{piece}", mesh)
    obj.location = offset
    bpy.context.scene.collection.objects.link(obj)
    return obj


def build_head(m):
    add_part("neck", "gorget", m["black"], lambda bm: tube(bm, (0, 0, 1.49), (0, 0, 1.6), 0.105, 0.085))
    helm = [
        (1.56, 0.1, 0.11, -0.01),
        (1.61, 0.128, 0.14, -0.005),
        (1.69, 0.138, 0.152, 0.0),
        (1.77, 0.133, 0.145, -0.01),
        (1.83, 0.115, 0.125, -0.015),
        (1.875, 0.08, 0.09, -0.02),
        (1.9, 0.035, 0.04, -0.02),
    ]
    add_part("head", "helm", m["crimson"], lambda bm: loft(bm, helm))
    add_part("head", "faceguard", m["black"], lambda bm: ellipsoid(bm, (0, 0.09, 1.68), (0.115, 0.075, 0.1)))
    add_part("head", "visor", m["glow"], lambda bm: box(bm, (0, 0.158, 1.72), (0.15, 0.03, 0.016)))

    def crest(bm):
        box(bm, (0, -0.02, 1.885), (0.022, 0.2, 0.05), bevel=0.008)

    add_part("head", "crest", m["gold"], crest)

    def horns(bm):
        for side in (-1, 1):
            tube(bm, (side * 0.11, 0.0, 1.79), (side * 0.2, -0.04, 1.9), 0.03, 0.012, segments=12)
            tube(bm, (side * 0.2, -0.04, 1.9), (side * 0.22, -0.1, 2.0), 0.012, 0.002, segments=12)

    add_part("head", "horns", m["black"], horns)


def build_torso(m):
    cuirass = [
        (1.17, 0.18, 0.125, 0.0),
        (1.22, 0.2, 0.14, 0.005),
        (1.3, 0.235, 0.158, 0.015),
        (1.39, 0.265, 0.168, 0.015),
        (1.46, 0.255, 0.155, 0.005),
        (1.51, 0.19, 0.125, 0.0),
        (1.535, 0.12, 0.09, 0.0),
    ]
    add_part("chest", "cuirass", m["crimson"], lambda bm: loft(bm, cuirass, segments=32))
    add_part("chest", "collar", m["gold"], lambda bm: tube(bm, (0, 0, 1.5), (0, 0, 1.545), 0.15, 0.12))

    # The mark: a gold ring around a glowing blood drop on a black field, big
    # enough to read from the follow camera.
    back = (0.0, -1.0, 0.0)
    add_part(
        "chest",
        "mark_field",
        m["black"],
        lambda bm: tube(bm, (0, -0.125, 1.39), (0, -0.172, 1.39), 0.112, 0.112, segments=32),
    )
    add_part("chest", "mark_ring", m["gold"], lambda bm: torus(bm, (0, -0.174, 1.39), back, 0.1, 0.012))
    add_part(
        "chest",
        "mark_drop",
        m["glow"],
        lambda bm: prism(bm, blood_drop(), (0, -0.176, 1.37), (-1, 0, 0), (0, 0, 1), 0.012),
    )

    fauld = [(1.05, 0.175, 0.125, 0.0), (1.12, 0.185, 0.13, 0.0), (1.2, 0.19, 0.13, 0.0)]
    add_part("spine", "fauld", m["black"], lambda bm: loft(bm, fauld, segments=28))
    add_part("hips", "pelvis", m["crimson"], lambda bm: ellipsoid(bm, (0, 0, 1.0), (0.19, 0.14, 0.1)))
    add_part("hips", "mail", m["black"], lambda bm: ellipsoid(bm, (0, 0, 0.93), (0.16, 0.12, 0.09)))
    add_part("hips", "belt", m["leather"], lambda bm: tube(bm, (0, 0, 1.03), (0, 0, 1.08), 0.195, 0.19, segments=28))
    add_part("hips", "buckle", m["gold"], lambda bm: box(bm, (0, 0.19, 1.055), (0.075, 0.02, 0.055), bevel=0.006))

    def scabbard(bm):
        tube(bm, (-0.23, 0.2, 1.0), (-0.27, -0.5, 0.66), 0.022, 0.018, segments=10)
        tube(bm, (-0.23, 0.2, 1.0), (-0.231, 0.18, 0.99), 0.027, 0.027, segments=10)

    add_part("hips", "scabbard", m["wrap"], scabbard)


def build_arm(m, suffix, side):
    x = side * 0.19

    def pauldron(bm):
        ellipsoid(bm, (side * 0.24, 0, 1.475), (0.14, 0.15, 0.11))

    add_part(f"upper_arm_{suffix}", "pauldron", m["crimson"], pauldron)
    add_part(
        f"upper_arm_{suffix}",
        "pauldron_lame",
        m["black"],
        lambda bm: ellipsoid(bm, (side * 0.26, 0, 1.4), (0.125, 0.135, 0.07)),
    )
    add_part(
        f"upper_arm_{suffix}",
        "rim",
        m["gold"],
        lambda bm: torus(bm, (side * 0.255, 0, 1.43), (side * 0.35, 0, -1), 0.125, 0.01),
    )
    add_part(f"upper_arm_{suffix}", "rerebrace", m["crimson"], lambda bm: limb(bm, x, [(1.42, 0.07), (1.32, 0.079), (1.2, 0.064)]))
    add_part(f"forearm_{suffix}", "couter", m["black"], lambda bm: ellipsoid(bm, (x, -0.01, 1.18), (0.07, 0.075, 0.07)))
    add_part(f"forearm_{suffix}", "vambrace", m["crimson"], lambda bm: limb(bm, x, [(1.16, 0.063), (1.08, 0.069), (0.97, 0.053)]))
    add_part(f"forearm_{suffix}", "cuff", m["gold"], lambda bm: tube(bm, (x, 0, 0.99), (x, 0, 0.94), 0.06, 0.072))
    add_part(
        f"hand_{suffix}",
        "gauntlet",
        m["black"],
        lambda bm: box(bm, (x, 0.005, 0.86), (0.078, 0.1, 0.12), bevel=0.02),
    )


def build_katana(m):
    # Held low in the right fist, blade forward and down with the edge toward the ground.
    fist = Vector((0.19, 0.005, 0.86))
    angle = math.radians(35)
    direction = Vector((0.0, math.cos(angle), -math.sin(angle)))
    up = Vector((0.0, math.sin(angle), math.cos(angle)))
    add_part("hand_r", "tsuka", m["wrap"], lambda bm: tube(bm, fist - direction * 0.17, fist + direction * 0.07, 0.017, 0.016, segments=10))
    add_part("hand_r", "kashira", m["gold"], lambda bm: tube(bm, fist - direction * 0.185, fist - direction * 0.165, 0.019, 0.019, segments=10))
    add_part("hand_r", "tsuba", m["gold"], lambda bm: tube(bm, fist + direction * 0.07, fist + direction * 0.082, 0.045, 0.045, segments=24))
    add_part("hand_r", "blade", m["blade"], lambda bm: katana_blade(bm, fist + direction * 0.08, direction, up))


def build_leg(m, suffix, side):
    x = side * 0.09
    add_part(f"thigh_{suffix}", "cuisse", m["crimson"], lambda bm: limb(bm, x, [(0.95, 0.084), (0.84, 0.09), (0.68, 0.08), (0.54, 0.066)]))

    def tasset(bm):
        tilt = Matrix.Rotation(math.radians(-side * 14), 4, "Y")
        box(bm, (side * 0.15, 0.03, 0.87), (0.035, 0.19, 0.19), bevel=0.012, rotation=tilt)

    add_part(f"thigh_{suffix}", "tasset", m["crimson"], tasset)
    add_part(f"thigh_{suffix}", "tasset_trim", m["gold"], lambda bm: box(bm, (side * 0.168, 0.03, 0.772), (0.03, 0.19, 0.02), bevel=0.006))
    add_part(f"shin_{suffix}", "poleyn", m["black"], lambda bm: ellipsoid(bm, (x, 0.025, 0.5), (0.07, 0.07, 0.075)))
    add_part(f"shin_{suffix}", "greave", m["crimson"], lambda bm: limb(bm, x, [(0.47, 0.062), (0.38, 0.072), (0.26, 0.06), (0.14, 0.05)], cy=-0.008))
    add_part(f"foot_{suffix}", "cuff", m["black"], lambda bm: tube(bm, (x, 0, 0.16), (x, 0, 0.08), 0.058, 0.064))

    def sabaton(bm):
        box(bm, (x, 0.05, 0.045), (0.11, 0.25, 0.09), bevel=0.025)
        ellipsoid(bm, (x, 0.16, 0.04), (0.055, 0.07, 0.04), segments=14, rings=8)

    add_part(f"foot_{suffix}", "sabaton", m["black"], sabaton)


def main():
    argv = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    if len(argv) != 1:
        raise SystemExit("usage: blood_knight.py -- <output.blend|.glb>")
    output = argv[0]

    bpy.ops.wm.read_factory_settings(use_empty=True)
    m = materials()
    build_head(m)
    build_torso(m)
    for suffix, side in (("l", -1.0), ("r", 1.0)):
        build_arm(m, suffix, side)
        build_leg(m, suffix, side)
    build_katana(m)

    if output.lower().endswith((".glb", ".gltf")):
        bpy.ops.export_scene.gltf(filepath=output, export_format="GLB", export_yup=True)
    else:
        bpy.ops.wm.save_as_mainfile(filepath=output)
    print(f"blood knight: {len(bpy.data.objects)} parts -> {output}")


main()
