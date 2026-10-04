// The compositor's passes (docs/ARCHITECTURE.md, color and scaling steps 1, 2 and 4).
//
// Resampling runs once per plane and axis: a horizontal pass into an intermediate texture,
// then a vertical pass. Values stay in code units of the picture's bit depth (16 to 235 is
// video black to white at 8 bits) until the last pass turns YUV into RGB.
//
// Every resource has its own binding number, so each pipeline binds only what its entry
// point uses.

struct Resample {
    // Size of the texture being read, in texels.
    src_size: vec2<i32>,
    // 0: resample along x, 1: along y.
    axis: u32,
    // For 16-bit integer sources: how far to shift right to get the code (6 for P010).
    code_shift: u32,
    // Source texels per output texel along the axis.
    step: f32,
    // Added to the source position, in source texels (chroma siting).
    offset: f32,
    // Kernel scale, max(step, 1): a downscale widens the kernel so it does not alias.
    scale: f32,
    // For normalized float sources: what turns a sample into a code (255 for 8-bit planes,
    // 1 for intermediates, which hold codes already).
    code_scale: f32,
}

struct Convert {
    // The picture's rectangle in the output; outside it is black.
    rect_min: vec2<i32>,
    rect_max: vec2<i32>,
    // R', G' and B' from (Y, U, V, 1) in codes.
    to_r: vec4<f32>,
    to_g: vec4<f32>,
    to_b: vec4<f32>,
}

@group(0) @binding(0) var<uniform> resample: Resample;
@group(0) @binding(1) var float_source: texture_2d<f32>;
@group(0) @binding(2) var uint_source: texture_2d<u32>;
@group(0) @binding(3) var<uniform> convert: Convert;
@group(0) @binding(4) var luma: texture_2d<f32>;
@group(0) @binding(5) var chroma: texture_2d<f32>;

// One triangle that covers the whole target.
@vertex
fn fullscreen(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

// Catmull-Rom, the cubic with B = 0 and C = 0.5.
fn catmull_rom(x: f32) -> f32 {
    let a = abs(x);
    if a < 1.0 {
        return (1.5 * a - 2.5) * a * a + 1.0;
    }
    if a < 2.0 {
        return ((-0.5 * a + 2.5) * a - 4.0) * a + 2.0;
    }
    return 0.0;
}

// Where output texel `position` reads from, in source texels (texel i is centered at i + 0.5).
fn source_center(position: vec4<f32>) -> f32 {
    let along = select(position.x, position.y, resample.axis == 1u);
    return along * resample.step + resample.offset;
}

// The source texel to read for tap `i`, clamped to the edge.
fn tap(position: vec4<f32>, i: i32) -> vec2<i32> {
    let size = select(resample.src_size.x, resample.src_size.y, resample.axis == 1u);
    let at = clamp(i, 0, size - 1);
    let here = vec2<i32>(position.xy);
    return select(vec2<i32>(at, here.y), vec2<i32>(here.x, at), resample.axis == 1u);
}

@fragment
fn resample_float(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let center = source_center(position);
    let radius = 2.0 * resample.scale;
    var sum = vec2<f32>(0.0);
    var total = 0.0;
    for (var i = i32(floor(center - 0.5 - radius)); i <= i32(ceil(center - 0.5 + radius)); i++) {
        let weight = catmull_rom((f32(i) + 0.5 - center) / resample.scale);
        sum += textureLoad(float_source, tap(position, i), 0).xy * resample.code_scale * weight;
        total += weight;
    }
    return vec4<f32>(sum / total, 0.0, 1.0);
}

@fragment
fn resample_uint(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let center = source_center(position);
    let radius = 2.0 * resample.scale;
    var sum = vec2<f32>(0.0);
    var total = 0.0;
    for (var i = i32(floor(center - 0.5 - radius)); i <= i32(ceil(center - 0.5 + radius)); i++) {
        let weight = catmull_rom((f32(i) + 0.5 - center) / resample.scale);
        let code = textureLoad(uint_source, tap(position, i), 0).xy >> vec2<u32>(resample.code_shift);
        sum += vec2<f32>(code) * weight;
        total += weight;
    }
    return vec4<f32>(sum / total, 0.0, 1.0);
}

@fragment
fn to_rgb(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy);
    if any(p < convert.rect_min) || any(p >= convert.rect_max) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let q = p - convert.rect_min;
    let yuv = vec4<f32>(textureLoad(luma, q, 0).x, textureLoad(chroma, q, 0).xy, 1.0);
    let rgb = vec3<f32>(dot(convert.to_r, yuv), dot(convert.to_g, yuv), dot(convert.to_b, yuv));
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
