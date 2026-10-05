// The compositor's passes (docs/ARCHITECTURE.md, color and scaling steps 1, 2 and 4).
//
// Each plane is resampled across in its own pass into a half-float intermediate; the last
// pass resamples both intermediates down and turns YUV into RGB, so only two intermediates
// exist per frame. A picture turned a quarter is read down its columns in the first pass
// (`transposed`), so its rows become the output's columns; a mirrored one is read with a
// negative step. Taps stay inside the crop. Values stay in code units of the picture's bit depth (16 to 235 is video
// black to white at 8 bits) until that last pass. Half floats hold them closely enough for
// 8-bit output: a 10-bit code is off by at most half a code, an eighth of an 8-bit one.
//
// Every resource has its own binding number, so each pipeline binds only what its entry
// point uses.

struct Resample {
    // The texels this pass reads along its axis: from src_min to src_max, the end excluded.
    src_min: i32,
    src_max: i32,
    // Source texels per output texel; negative where the picture is mirrored.
    step: f32,
    // The source position of the output's edge, in source texels (crop and chroma siting).
    offset: f32,
    // Kernel scale, max(|step|, 1): a downscale widens the kernel so it does not alias.
    scale: f32,
    // 1 when this pass reads down the plane's columns.
    transposed: i32,
}

struct Convert {
    // The picture's rectangle in the output; outside it is black.
    rect_min: vec2<i32>,
    rect_max: vec2<i32>,
    // R', G' and B' from (Y, U, V, 1) in codes.
    to_r: vec4<f32>,
    to_g: vec4<f32>,
    to_b: vec4<f32>,
    // Resampling down: the rows of each intermediate that the picture shows (from .x to .y,
    // .y excluded), and per plane the source rows per output row, the source position of the
    // output's top edge and the kernel scale, as in Resample.
    luma_rows: vec2<i32>,
    chroma_rows: vec2<i32>,
    luma_step: f32,
    luma_offset: f32,
    luma_scale: f32,
    chroma_step: f32,
    chroma_offset: f32,
    chroma_scale: f32,
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
    return position.x * resample.step + resample.offset;
}

// The source texel to read for tap `i`, clamped to the shown part.
fn tap(position: vec4<f32>, i: i32) -> vec2<i32> {
    let along = clamp(i, resample.src_min, resample.src_max - 1);
    let across = i32(position.y);
    if resample.transposed != 0 {
        return vec2<i32>(across, along);
    }
    return vec2<i32>(along, across);
}

// 8-bit planes are normalized, so a sample times 255 is its code.
@fragment
fn resample_float(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let center = source_center(position);
    let radius = 2.0 * resample.scale;
    var sum = vec2<f32>(0.0);
    var total = 0.0;
    for (var i = i32(floor(center - 0.5 - radius)); i <= i32(ceil(center - 0.5 + radius)); i++) {
        let weight = catmull_rom((f32(i) + 0.5 - center) / resample.scale);
        sum += textureLoad(float_source, tap(position, i), 0).xy * 255.0 * weight;
        total += weight;
    }
    return vec4<f32>(sum / total, 0.0, 1.0);
}

// P010 planes keep each 10-bit code in the top bits of a 16-bit sample.
@fragment
fn resample_uint(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let center = source_center(position);
    let radius = 2.0 * resample.scale;
    var sum = vec2<f32>(0.0);
    var total = 0.0;
    for (var i = i32(floor(center - 0.5 - radius)); i <= i32(ceil(center - 0.5 + radius)); i++) {
        let weight = catmull_rom((f32(i) + 0.5 - center) / resample.scale);
        let code = textureLoad(uint_source, tap(position, i), 0).xy >> vec2<u32>(6u);
        sum += vec2<f32>(code) * weight;
        total += weight;
    }
    return vec4<f32>(sum / total, 0.0, 1.0);
}

// Resamples column `x` of `plane` down to output row `row` (centered at row + 0.5).
fn down(plane: texture_2d<f32>, x: i32, row: f32, rows: vec2<i32>, step: f32, offset: f32, scale: f32) -> vec2<f32> {
    let center = row * step + offset;
    let radius = 2.0 * scale;
    var sum = vec2<f32>(0.0);
    var total = 0.0;
    for (var i = i32(floor(center - 0.5 - radius)); i <= i32(ceil(center - 0.5 + radius)); i++) {
        let weight = catmull_rom((f32(i) + 0.5 - center) / scale);
        sum += textureLoad(plane, vec2<i32>(x, clamp(i, rows.x, rows.y - 1)), 0).xy * weight;
        total += weight;
    }
    return sum / total;
}

@fragment
fn to_rgb(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy);
    if any(p < convert.rect_min) || any(p >= convert.rect_max) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let q = p - convert.rect_min;
    let row = f32(q.y) + 0.5;
    let y = down(luma, q.x, row, convert.luma_rows, convert.luma_step, convert.luma_offset, convert.luma_scale).x;
    let uv = down(chroma, q.x, row, convert.chroma_rows, convert.chroma_step, convert.chroma_offset, convert.chroma_scale);
    let yuv = vec4<f32>(y, uv, 1.0);
    let rgb = vec3<f32>(dot(convert.to_r, yuv), dot(convert.to_g, yuv), dot(convert.to_b, yuv));
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
