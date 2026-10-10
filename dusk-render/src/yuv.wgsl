// The export pass (docs/ARCHITECTURE.md, "Color and scaling"): R'G'B' to limited-range BT.709
// Y'CbCr 4:2:0. Luma is computed per pixel; chroma is resampled from the R'G'B' frame down to
// half size each way with Catmull-Rom widened for the 2:1 step, at MPEG-2 (left) siting: on
// the even luma columns, halfway between two luma rows. Resampling R'G'B' and then converting
// equals converting and then resampling, because the conversion is linear.

@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var<uniform> size: vec2<i32>;

const KR: f32 = 0.2126;
const KB: f32 = 0.0722;
const KG: f32 = 1.0 - KR - KB;

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

fn rgb_at(p: vec2<i32>) -> vec3<f32> {
    return textureLoad(frame, clamp(p, vec2<i32>(0), size - 1), 0).rgb;
}

// Y' in codes of the 8-bit target, normalized for an R8Unorm target.
@fragment
fn to_luma(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let rgb = rgb_at(vec2<i32>(position.xy));
    let y = dot(rgb, vec3<f32>(KR, KG, KB));
    return vec4<f32>((16.0 + 219.0 * y) / 255.0, 0.0, 0.0, 1.0);
}

// Cb and Cr for an Rg8Unorm target.
@fragment
fn to_chroma(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    // Where this chroma sample sits, in frame pixels (pixel i is centered at i + 0.5).
    let center = vec2<f32>(floor(position.x) * 2.0 + 0.5, floor(position.y) * 2.0 + 1.0);
    // Two frame pixels per chroma sample: the kernel is twice as wide, a radius of 4.
    let scale = 2.0;
    let radius = 2.0 * scale;
    let low = vec2<i32>(floor(center - 0.5 - radius));
    let high = vec2<i32>(ceil(center - 0.5 + radius));
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var y = low.y; y <= high.y; y++) {
        let wy = catmull_rom((f32(y) + 0.5 - center.y) / scale);
        for (var x = low.x; x <= high.x; x++) {
            let weight = wy * catmull_rom((f32(x) + 0.5 - center.x) / scale);
            sum += rgb_at(vec2<i32>(x, y)) * weight;
            total += weight;
        }
    }
    let rgb = sum / total;
    let y = dot(rgb, vec3<f32>(KR, KG, KB));
    let cb = (rgb.b - y) / (2.0 * (1.0 - KB));
    let cr = (rgb.r - y) / (2.0 * (1.0 - KR));
    return vec4<f32>((128.0 + 224.0 * cb) / 255.0, (128.0 + 224.0 * cr) / 255.0, 0.0, 1.0);
}
