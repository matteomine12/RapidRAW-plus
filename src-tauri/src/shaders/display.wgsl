struct Transform {
    rect: vec4<f32>,
    clip: vec4<f32>,
    window: vec2<f32>,
    image_size: vec2<f32>,
    texture_size: vec2<f32>,
    pixelated: f32,
    hdr_scale: f32,
    bg_primary: vec4<f32>,
    bg_secondary: vec4<f32>,
    original_size: vec2<f32>,
    split_origin: vec2<f32>,
    split_normal: vec2<f32>,
    split_offset: vec2<f32>,
    extra: vec4<f32>,
};

@group(0) @binding(0) var<uniform> transform: Transform;
@group(0) @binding(1) var tex: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var original_tex: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) id: u32) -> VertexOutput {
    let uvs = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0)
    );
    let pos = uvs[id];

    let uv_x = transform.clip.x + pos.x * transform.clip.z;
    let uv_y = transform.clip.y + pos.y * transform.clip.w;

    let half_pixel_x = 0.5;
    let half_pixel_y = 0.5;
    let outset_x = (pos.x * 2.0 - 1.0) * half_pixel_x;
    let outset_y = (pos.y * 2.0 - 1.0) * half_pixel_y;

    let screen_x = uv_x + outset_x;
    let screen_y = uv_y + outset_y;

    let ndc_x = (screen_x / transform.window.x) * 2.0 - 1.0;
    let ndc_y = 1.0 - (screen_y / transform.window.y) * 2.0;

    var out: VertexOutput;
    out.pos = vec4<f32>(ndc_x, ndc_y, 0.0, 1.0);

    out.uv = vec2<f32>(
        (uv_x - transform.rect.x) / transform.rect.z,
        (uv_y - transform.rect.y) / transform.rect.w
    );

    return out;
}

fn sample_layer(layer: texture_2d<f32>, uv: vec2<f32>, image_size: vec2<f32>, texture_size: vec2<f32>) -> vec4<f32> {
    let adjusted_uv = uv * (image_size / texture_size);
    let half_texel = vec2<f32>(0.5, 0.5) / texture_size;

    let min_uv = half_texel;
    let max_uv = (image_size / texture_size) - half_texel;

    if (transform.pixelated > 0.5) {
        let texel_coords = floor(adjusted_uv * texture_size);
        let nearest_uv = (texel_coords + vec2<f32>(0.5, 0.5)) / texture_size;

        let clamped_nearest = clamp(nearest_uv, min_uv, max_uv);
        return textureSampleLevel(layer, samp, clamped_nearest, 0.0);
    } else {
        let clamped_uv = clamp(adjusted_uv, min_uv, max_uv);
        return textureSampleLevel(layer, samp, clamped_uv, 0.0);
    }
}

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lower = c / 12.92;
    let higher = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(higher, lower, c <= vec3<f32>(0.04045));
}

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let v = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let lower = v * 12.92;
    let higher = 1.055 * pow(v, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(higher, lower, v <= vec3<f32>(0.0031308));
}

// Linear Rec.709 light in scRGB units (1.0 = 80 nits) to PQ-encoded Rec.2020, the signal of an HDR10 surface.
fn hdr10_pq(scrgb: vec3<f32>) -> vec3<f32> {
    let nits = max(scrgb, vec3<f32>(0.0)) * 80.0;
    let rec2020 = vec3<f32>(
        dot(nits, vec3<f32>(0.627404, 0.329283, 0.043313)),
        dot(nits, vec3<f32>(0.069097, 0.919540, 0.011362)),
        dot(nits, vec3<f32>(0.016391, 0.088013, 0.895595)),
    );
    let y = pow(clamp(rec2020 / 10000.0, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(0.1593017578125));
    return pow((vec3<f32>(0.8359375) + 18.8515625 * y) / (vec3<f32>(1.0) + 18.6875 * y), vec3<f32>(78.84375));
}

// Converts a sampled texel into the surface's encoding: sRGB for an SDR surface, scaled linear light
// (scRGB, 1.0 = 80 nits) or PQ Rec.2020 (HDR10, `extra.y`) for an HDR one. `src_linear` marks textures that hold linear light.
fn to_surface(c: vec4<f32>, src_linear: bool) -> vec4<f32> {
    if (transform.hdr_scale > 0.0) {
        var lin = c.rgb;
        if (!src_linear) {
            lin = srgb_to_linear(lin);
        }
        let scrgb = lin * transform.hdr_scale;
        if (transform.extra.y > 0.5) {
            return vec4<f32>(hdr10_pq(scrgb), c.a);
        }
        return vec4<f32>(scrgb, c.a);
    }
    if (src_linear) {
        return vec4<f32>(linear_to_srgb(c.rgb), c.a);
    }
    return c;
}

fn background(c: vec4<f32>) -> vec4<f32> {
    return to_surface(c, false);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let is_original = dot(in.pos.xy - transform.split_origin, transform.split_normal) < 0.0;
    let uv = select(in.uv, in.uv + transform.split_offset / transform.rect.zw, is_original);

    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0) {
        return background(transform.bg_secondary);
    }

    let src_linear = transform.extra.x > 0.5;
    if (is_original) {
        return to_surface(sample_layer(original_tex, uv, transform.original_size, transform.original_size), src_linear);
    }
    return to_surface(sample_layer(tex, uv, transform.image_size, transform.texture_size), src_linear);
}
