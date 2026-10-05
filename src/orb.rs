//! The orb, drawn per pixel on the CPU like a small fragment shader: a dark glass
//! sphere with light flowing inside it, a lit rim, a specular highlight, a soft
//! halo and a turning dotted ring. Voice brightens and stirs it; writing turns it gold.
//!
//! Output is BGRA with straight alpha, which is what GPUI's sprite atlas takes.

type V3 = [f32; 3];

fn mix(a: V3, b: V3, t: f32) -> V3 {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn hex(c: u32) -> V3 {
    [((c >> 16) & 255) as f32 / 255., ((c >> 8) & 255) as f32 / 255., (c & 255) as f32 / 255.]
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub struct Style {
    pub voice: f32,
    pub writing: bool,
}

/// Render a `size`×`size` frame at time `t` (seconds).
pub fn render(size: usize, t: f32, style: &Style) -> Vec<u8> {
    let graphite = hex(0x15161b);
    let slate = hex(0x6f8493);
    let gold = hex(0xd9a86a);
    let cream = hex(0xede0c4);
    let v = style.voice.clamp(0.0, 1.0);
    let writing = style.writing;

    let radius = 0.42 + 0.06 * v - if writing { 0.03 } else { 0.0 };
    let flow = t * if writing { 1.6 } else { 0.35 + 0.9 * v };
    let glow_strength = if writing { 0.55 } else { 0.25 + 0.6 * v };
    let ring_r = radius * (1.42 + 0.12 * v);
    let ring_spin = t * if writing { 2.4 } else { 0.25 + 0.6 * v };
    let light = {
        let l: V3 = [-0.45, -0.6, 0.66];
        let n = (l[0] * l[0] + l[1] * l[1] + l[2] * l[2]).sqrt();
        l.map(|x| x / n)
    };

    let mut out = vec![0u8; size * size * 4];
    let px = 2.0 / size as f32;
    for j in 0..size {
        for i in 0..size {
            let x = (i as f32 + 0.5) * px - 1.0;
            let y = (j as f32 + 0.5) * px - 1.0;
            let d = (x * x + y * y).sqrt();

            let (mut rgb, mut a);
            if d < radius + px {
                // Inside the sphere.
                let (nx, ny) = (x / radius, y / radius);
                let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                // Light flowing inside: silky ribbons from layered, self-warping sines.
                let (qx, qy, qz) = (nx * 2.2, ny * 2.2, nz * 2.2);
                let w1 = (qx * 1.7 + flow).sin() + (qy * 2.3 - flow * 0.8).sin();
                let w2 = (qz * 1.9 + flow * 1.2 + w1).sin();
                let ribbon = 0.5 + 0.5 * (qx * 2.1 + qy * 1.3 + w2 * 1.6 + flow * 0.6).sin();
                let veil = 0.5 + 0.5 * (qy * 2.7 - qz * 1.1 + w1 * 1.3 - flow * 0.9).sin();
                let energy = 0.35 + 0.65 * v.max(if writing { 0.75 } else { 0.0 });
                let gold_light = smoothstep(0.62, 1.0, ribbon) * energy;
                let slate_light = smoothstep(0.5, 1.0, veil) * (0.25 + 0.35 * energy);
                let core_light = smoothstep(0.75, 1.0, ribbon * veil) * energy;
                rgb = graphite;
                let glow = |c: V3, k: f32, rgb: V3| -> V3 { [rgb[0] + c[0] * k, rgb[1] + c[1] * k, rgb[2] + c[2] * k] };
                rgb = glow(if writing { gold } else { slate }, slate_light * 0.55, rgb);
                rgb = glow(gold, gold_light * 0.85, rgb);
                rgb = glow(cream, core_light * 0.7, rgb);
                // Lit glass: soft lambert, a cream rim, a sharp highlight.
                let lambert = (nx * light[0] + ny * light[1] + nz * light[2]).max(0.0);
                rgb = rgb.map(|c| c * (0.55 + 0.6 * lambert));
                let fresnel = (1.0 - nz).powf(2.6);
                rgb = mix(rgb, cream, (fresnel * (0.55 + 0.4 * v)).min(1.0));
                let h = [light[0], light[1], light[2] + 1.0];
                let hn = (h[0] * h[0] + h[1] * h[1] + h[2] * h[2]).sqrt();
                let spec = ((nx * h[0] + ny * h[1] + nz * h[2]) / hn).max(0.0).powf(48.0);
                rgb = rgb.map(|c| (c + spec * 0.9).min(1.0));
                a = 1.0 - smoothstep(radius - px, radius + px, d);
                // Blend the halo under the antialiased edge.
                if a < 1.0 {
                    let halo = glow_strength * 0.9;
                    rgb = mix(gold, rgb, a);
                    a = a + (1.0 - a) * halo;
                }
            } else {
                // Halo: warm light falling off from the rim.
                let fall = (-(d - radius) * 9.0).exp();
                a = fall * glow_strength;
                rgb = mix(gold, cream, fall);
                // The dotted ring, turning.
                let ring = 1.0 - smoothstep(0.0, px * 1.4, (d - ring_r).abs());
                if ring > 0.0 {
                    let angle = y.atan2(x) + ring_spin;
                    let dots = 0.5 + 0.5 * (angle * 48.0).cos();
                    let dot = smoothstep(0.55, 0.9, dots) * ring * (0.35 + 0.4 * v);
                    rgb = mix(rgb, cream, dot / (a + dot).max(1e-3));
                    a = a + dot * (1.0 - a);
                }
            }

            let o = (j * size + i) * 4;
            let a = a.clamp(0.0, 1.0);
            out[o] = (rgb[2].clamp(0.0, 1.0) * 255.0) as u8;
            out[o + 1] = (rgb[1].clamp(0.0, 1.0) * 255.0) as u8;
            out[o + 2] = (rgb[0].clamp(0.0, 1.0) * 255.0) as u8;
            out[o + 3] = (a * 255.0) as u8;
        }
    }
    out
}
