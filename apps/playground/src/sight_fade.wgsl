// StandardMaterial with a soft cut-out along the camera's line of sight to a focus point. The
// host switches opaque materials to blending while the cut-out is active.
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_view_bindings::view,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    pbr_types,
}

struct SightFade {
    focus: vec3<f32>,
    // World radius of the cut-out cylinder: occluders near the camera, which fill the view, are
    // cut across all of it; near the focus it clears the character.
    radius: f32,
    strength: f32,
    min_opacity: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> sight: SightFade;

fn sight_visibility(world_position: vec3<f32>) -> f32 {
    let to_focus = sight.focus - view.world_position;
    let span = length(to_focus);
    if sight.strength <= 0.0 || span < 1e-4 {
        return 1.0;
    }
    let direction = to_focus / span;
    let offset = world_position - view.world_position;
    let along = dot(offset, direction);
    let across = length(offset - direction * along);
    // A wide falloff: big surfaces thin out toward the hole instead of showing an edge.
    let inside = (1.0 - smoothstep(sight.radius * 0.15, sight.radius, across))
        * step(0.0, along);
    // Only what lies in front of the focus: the ground it stands on stays.
    let in_front = 1.0 - smoothstep(span - 0.6, span - 0.2, along);
    return mix(1.0, sight.min_opacity, inside * in_front * sight.strength);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    pbr_input.material.base_color.a *= sight_visibility(in.world_position.xyz);

    var out: FragmentOutput;
    if (pbr_input.material.flags & pbr_types::STANDARD_MATERIAL_FLAGS_UNLIT_BIT) == 0u {
        out.color = apply_pbr_lighting(pbr_input);
    } else {
        out.color = pbr_input.material.base_color;
    }
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
