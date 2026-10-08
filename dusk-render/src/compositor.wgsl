// The compositor's passes (docs/ARCHITECTURE.md, color and scaling steps 1 to 4).
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
    // Color step 3: 0 skips it (BT.709 SDR), 1 is SDR with other primaries, 2 is PQ, 3 is
    // HLG. For SDR, `srgb` says whether the values use the sRGB curve or BT.1886.
    color_step: i32,
    srgb: i32,
    // HDR tone mapping (BT.2390): the source peak in nits and as a PQ value, the knee and
    // SDR white relative to that peak in PQ.
    peak_nits: f32,
    peak_pq: f32,
    knee: f32,
    max_lum: f32,
    // The gamut matrix to BT.709, row by row.
    gamut_r: vec4<f32>,
    gamut_g: vec4<f32>,
    gamut_b: vec4<f32>,
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

// The sRGB curve and BT.1886 (gamma 2.4), both ways, as dusk-core's color module has them.
fn sdr_to_linear(value: vec3<f32>) -> vec3<f32> {
    let v = max(value, vec3<f32>(0.0));
    if convert.srgb != 0 {
        return select(pow((v + 0.055) / 1.055, vec3<f32>(2.4)), v / 12.92, v <= vec3<f32>(0.04045));
    }
    return pow(v, vec3<f32>(2.4));
}

fn linear_to_sdr(light: vec3<f32>) -> vec3<f32> {
    let l = max(light, vec3<f32>(0.0));
    if convert.srgb != 0 {
        return select(1.055 * pow(l, vec3<f32>(1.0 / 2.4)) - 0.055, l * 12.92, l <= vec3<f32>(0.0031308));
    }
    return pow(l, vec3<f32>(1.0 / 2.4));
}

// SMPTE ST 2084.
const PQ_M1: f32 = 0.1593017578125;
const PQ_M2: f32 = 78.84375;
const PQ_C1: f32 = 0.8359375;
const PQ_C2: f32 = 18.8515625;
const PQ_C3: f32 = 18.6875;

fn pq_to_nits(value: vec3<f32>) -> vec3<f32> {
    let p = pow(max(value, vec3<f32>(0.0)), vec3<f32>(1.0 / PQ_M2));
    return 10000.0 * pow(max(p - PQ_C1, vec3<f32>(0.0)) / (PQ_C2 - PQ_C3 * p), vec3<f32>(1.0 / PQ_M1));
}

fn nits_to_pq(nits: f32) -> f32 {
    let p = pow(max(nits / 10000.0, 0.0), PQ_M1);
    return pow((PQ_C1 + PQ_C2 * p) / (1.0 + PQ_C3 * p), PQ_M2);
}

fn pq_value_to_nits(value: f32) -> f32 {
    return pq_to_nits(vec3<f32>(value)).x;
}

// ARIB STD-B67: scene light from the signal, then the OOTF at the 1000-nit reference.
fn hlg_to_display(value: vec3<f32>) -> vec3<f32> {
    let v = max(value, vec3<f32>(0.0));
    let scene = select((exp((v - 0.55991073) / 0.17883277) + 0.28466892) / 12.0, v * v / 3.0, v <= vec3<f32>(0.5));
    let luminance = dot(vec3<f32>(0.2627, 0.6780, 0.0593), scene);
    return 1000.0 * pow(max(luminance, 0.0), 0.2) * scene;
}

// BT.2390's EETF on a PQ value, down to SDR white.
fn eetf(value: f32) -> f32 {
    let normalized = clamp(value / convert.peak_pq, 0.0, 1.0);
    if normalized < convert.knee {
        return normalized * convert.peak_pq;
    }
    let t = (normalized - convert.knee) / (1.0 - convert.knee);
    let t2 = t * t;
    let t3 = t2 * t;
    let mapped = (2.0 * t3 - 3.0 * t2 + 1.0) * convert.knee + (t3 - 2.0 * t2 + t) * (1.0 - convert.knee) + (-2.0 * t3 + 3.0 * t2) * convert.max_lum;
    return mapped * convert.peak_pq;
}

// Tone maps display light in nits to linear SDR light where 1 is 100 nits: max(R, G, B)
// through the EETF, all three scaled with it.
fn tone_map(nits_in: vec3<f32>) -> vec3<f32> {
    let nits = clamp(nits_in, vec3<f32>(0.0), vec3<f32>(convert.peak_nits));
    let brightest = max(nits.r, max(nits.g, nits.b));
    if brightest <= 0.0 {
        return vec3<f32>(0.0);
    }
    let mapped = pq_value_to_nits(eetf(nits_to_pq(brightest)));
    return nits * (mapped / brightest / 100.0);
}

// Step 3: for sources with other primaries or HDR, linearize, tone map HDR, move into BT.709
// with clipping, and encode again.
fn step_3(rgb: vec3<f32>) -> vec3<f32> {
    var light: vec3<f32>;
    if convert.color_step == 1 {
        light = sdr_to_linear(rgb);
    } else if convert.color_step == 2 {
        light = tone_map(pq_to_nits(rgb));
    } else {
        light = tone_map(hlg_to_display(rgb));
    }
    let bt709 = clamp(
        vec3<f32>(dot(convert.gamut_r.xyz, light), dot(convert.gamut_g.xyz, light), dot(convert.gamut_b.xyz, light)),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    if convert.color_step == 1 {
        return linear_to_sdr(bt709);
    }
    // HDR is shown as SDR video: inverse BT.1886.
    return pow(bt709, vec3<f32>(1.0 / 2.4));
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
    let rgb = clamp(
        vec3<f32>(dot(convert.to_r, yuv), dot(convert.to_g, yuv), dot(convert.to_b, yuv)),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    if convert.color_step == 0 {
        return vec4<f32>(rgb, 1.0);
    }
    return vec4<f32>(step_3(rgb), 1.0);
}
