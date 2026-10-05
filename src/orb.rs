//! The orb, drawn per pixel on the CPU like a small fragment shader: a living
//! atmosphere rather than a ball. Venus-like cloud bands wrap a slowly turning
//! globe, warped by flowing turbulence; the limb dissolves into a lit haze and the
//! silhouette itself drifts. Voice stirs and brightens it; writing turns it gold.
//!
//! Rendered at screen points, not retina pixels: the clouds are soft, so the GPU's
//! upscale costs nothing visible and the frame is a quarter of the work.
//!
//! Output is BGRA with straight alpha, which is what GPUI's sprite atlas takes.

type V3 = [f32; 3];

fn mix(a: V3, b: V3, t: f32) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn add(a: V3, b: V3, k: f32) -> V3 {
    [a[0] + b[0] * k, a[1] + b[1] * k, a[2] + b[2] * k]
}

fn hex(c: u32) -> V3 {
    [((c >> 16) & 255) as f32 / 255., ((c >> 8) & 255) as f32 / 255., (c & 255) as f32 / 255.]
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---- smooth 3D gradient noise (Perlin) ----

const PERM: [u8; 256] = {
    // A fixed shuffle of 0..=255 (xorshift, seeded), built at compile time.
    let mut p = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        p[i] = i as u8;
        i += 1;
    }
    let mut s: u32 = 0x9e37_79b9;
    let mut i = 255;
    while i > 0 {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        let j = (s % (i as u32 + 1)) as usize;
        let t = p[i];
        p[i] = p[j];
        p[j] = t;
        i -= 1;
    }
    p
};

fn perm(i: i32) -> usize {
    PERM[(i & 255) as usize] as usize
}

fn grad(h: usize, x: f32, y: f32, z: f32) -> f32 {
    match h & 15 {
        0 => x + y,
        1 => -x + y,
        2 => x - y,
        3 => -x - y,
        4 => x + z,
        5 => -x + z,
        6 => x - z,
        7 => -x - z,
        8 => y + z,
        9 => -y + z,
        10 => y - z,
        11 => -y - z,
        12 => x + y,
        13 => -y + z,
        14 => -x + y,
        _ => -y - z,
    }
}

/// Perlin noise, roughly -1..1.
fn noise(x: f32, y: f32, z: f32) -> f32 {
    let (xi, yi, zi) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    let (xf, yf, zf) = (x - x.floor(), y - y.floor(), z - z.floor());
    let fade = |t: f32| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
    let (u, v, w) = (fade(xf), fade(yf), fade(zf));
    let p = |i: i32| perm(i) as i32;
    let a = p(xi) + yi;
    let aa = p(a) + zi;
    let ab = p(a + 1) + zi;
    let b = p(xi + 1) + yi;
    let ba = p(b) + zi;
    let bb = p(b + 1) + zi;
    let l = |a: f32, b: f32, t: f32| a + (b - a) * t;
    l(
        l(
            l(grad(perm(aa), xf, yf, zf), grad(perm(ba), xf - 1., yf, zf), u),
            l(grad(perm(ab), xf, yf - 1., zf), grad(perm(bb), xf - 1., yf - 1., zf), u),
            v,
        ),
        l(
            l(grad(perm(aa + 1), xf, yf, zf - 1.), grad(perm(ba + 1), xf - 1., yf, zf - 1.), u),
            l(
                grad(perm(ab + 1), xf, yf - 1., zf - 1.),
                grad(perm(bb + 1), xf - 1., yf - 1., zf - 1.),
                u,
            ),
            v,
        ),
        w,
    )
}

fn fbm(mut x: f32, mut y: f32, mut z: f32, octaves: u32) -> f32 {
    let (mut sum, mut amp) = (0.0, 0.5);
    for _ in 0..octaves {
        sum += amp * noise(x, y, z);
        x = x * 2.03 + 1.7;
        y = y * 2.03 - 3.1;
        z = z * 2.03 + 0.9;
        amp *= 0.5;
    }
    sum
}

pub struct Style {
    pub voice: f32,
    pub writing: bool,
}

/// Render a `size`×`size` frame at time `t` (seconds).
pub fn render(size: usize, t: f32, style: &Style) -> Vec<u8> {
    let night = hex(0x14151b);
    let umber = hex(0x4a3524);
    let ochre = hex(0xb07d45);
    let gold = hex(0xd9a86a);
    let cream = hex(0xf2e6cc);
    let lavender = hex(0x9a94b4);
    let v = style.voice.clamp(0.0, 1.0);
    let writing = style.writing;

    let radius = 0.5 + 0.05 * v - if writing { 0.03 } else { 0.0 };
    let spin = t * if writing { 0.9 } else { 0.08 + 0.25 * v };
    let churn = t * if writing { 0.5 } else { 0.12 + 0.35 * v };
    let turbulence = 0.9 + 1.4 * v + if writing { 0.6 } else { 0.0 };
    let brightness = 0.75 + 0.45 * v + if writing { 0.25 } else { 0.0 };
    let haze = 0.35 + 0.55 * v + if writing { 0.2 } else { 0.0 };
    let light: V3 = {
        let l: V3 = [-0.55, -0.45, 0.7];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        l.map(|x| x / n)
    };

    let px = 2.0 / size as f32;
    let mut out = vec![0u8; size * size * 4];
    out.chunks_mut(size * 4).enumerate().for_each(|(j, row)| {
        let y = (j as f32 + 0.5) * px - 1.0;
        for i in 0..size {
            let x = (i as f32 + 0.5) * px - 1.0;
            let d = (x * x + y * y).sqrt();
            // The silhouette drifts: the edge breathes with slow noise around the rim.
            let angle = y.atan2(x);
            let wisp = noise(angle.cos() * 1.6 + churn * 0.7, angle.sin() * 1.6, t * 0.15) * (0.012 + 0.02 * v);
            let edge = radius * (1.0 + wisp);

            let mut rgb = [0.0f32; 3];
            let mut a = 0.0f32;
            if d < edge * 1.02 {
                let (nx, ny) = (x / edge, y / edge);
                let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                // Spherical coordinates on a turning globe.
                let lon = nx.atan2(nz) * 0.9 + spin;
                let lat = ny.clamp(-1.0, 1.0).asin();
                let (sx, sy, sz) = (lon.cos(), lat * 2.2, lon.sin());
                // Domain-warped clouds, broad and soft, stretched into latitude sweeps.
                let wx = fbm(sx * 0.9, sy * 0.8 + churn, sz * 0.9, 2);
                let wy = fbm(sx * 0.9 + 5.2, sy * 0.8 - churn * 0.8, sz * 0.9 + 1.3, 2);
                let clouds = fbm(
                    sx * 1.1 + wx * turbulence,
                    sy * 1.9 + wy * turbulence * 0.7 + churn * 0.4,
                    sz * 1.1 + wy * turbulence,
                    2,
                ) * 0.5
                    + 0.5;
                let sweep = 0.5 + 0.5 * (lat * 4.0 + wx * 2.0 * turbulence + churn).sin();
                let c = (clouds * 0.75 + sweep * 0.25).clamp(0.0, 1.0);

                // A pale, luminous body; ochre sweeps; lavender in the deepest folds.
                rgb = mix(ochre, gold, smoothstep(0.2, 0.55, c));
                rgb = mix(rgb, cream, smoothstep(0.5, 0.85, c));
                rgb = mix(rgb, umber, smoothstep(0.32, 0.1, c) * 0.45);
                rgb = mix(rgb, lavender, smoothstep(0.4, 0.15, c) * 0.35);
                if writing {
                    rgb = mix(rgb, gold, 0.3);
                }
                // Soft daylight with a long twilight; the night side glows lavender, not black.
                let lambert = nx * light[0] + ny * light[1] + nz * light[2];
                let day = smoothstep(-0.5, 0.7, lambert);
                let dusk = mix(mix(lavender, night, 0.35), rgb, 0.25);
                rgb = mix(dusk, rgb, 0.35 + 0.65 * day).map(|ch| ch * brightness);
                // Bright haze at the limb, like sunlight scattering through thick air.
                // Strongest right at the edge, and lit on the night side too, so the
                // rim never reads as a dark outline.
                let limb = (1.0 - nz).powf(1.3);
                rgb = add(rgb, mix(gold, cream, 0.6), limb * (0.55 + 0.4 * v) * (0.7 + 0.3 * day));
                // Gaseous: the body is faintly translucent and dissolves at the edge.
                a = smoothstep(edge * 1.02, edge * 0.9, d) * (0.88 + 0.12 * c);
            }
            // Outer haze: warm light falling off past the edge.
            let past = (d - edge).max(0.0);
            let fall = 0.6 * (-past * 11.0).exp() + 0.4 * (-past * 4.0).exp();
            let halo = fall * haze * smoothstep(edge * 0.9, edge * 1.0, d);
            if halo > 0.0 {
                let hc = mix(gold, cream, fall * 0.6);
                let total = a + halo * (1.0 - a);
                rgb = mix(hc, rgb, if total > 0.0 { a / total } else { 0.0 });
                a = total;
            }

            // Keep bright light from clipping channel by channel (which shifts the hue
            // toward cyan): scale the whole colour down instead.
            let peak = rgb[0].max(rgb[1]).max(rgb[2]);
            if peak > 1.0 {
                rgb = rgb.map(|ch| ch / peak);
            }
            let o = i * 4;
            row[o] = (rgb[2].clamp(0.0, 1.0) * 255.0) as u8;
            row[o + 1] = (rgb[1].clamp(0.0, 1.0) * 255.0) as u8;
            row[o + 2] = (rgb[0].clamp(0.0, 1.0) * 255.0) as u8;
            row[o + 3] = (a.clamp(0.0, 1.0) * 255.0) as u8;
        }
    });
    out
}
