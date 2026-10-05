//! The orb on the GPU: our own Metal shader in a small native overlay window.
//!
//! Light woven through a sphere as thin glowing sheets (a warped gyroid field),
//! wound in shells around a leaning axis with a soft spine of light through it.
//! Talking winds it tighter and brighter; when you let go it releases, the
//! winding loosening and racing outward while Heyra writes.
//!
//! Original shader. It shares only the general technique (ray marching through
//! emissive fog) with Orbkit's examples, none of their code.

#![allow(unexpected_cfgs)] // objc 0.2's macros check a cfg this crate doesn't declare

use cocoa::appkit::{NSBackingStoreType, NSScreen, NSView, NSWindowStyleMask};
use cocoa::base::{NO, YES, id, nil};
use cocoa::foundation::{NSPoint, NSRect, NSSize};
use metal::{
    CommandQueue, CompileOptions, Device, MTLClearColor, MTLLoadAction, MTLOrigin, MTLPixelFormat,
    MTLPrimitiveType, MTLRegion, MTLSize, MTLStorageMode, MTLStoreAction, MTLTextureUsage, MetalLayer,
    RenderPassDescriptor, RenderPipelineDescriptor, RenderPipelineState, TextureDescriptor, TextureRef,
};
use core_graphics_types::geometry::CGSize;
use objc::{class, msg_send, sel, sel_impl};

const SHADER: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct VOut { float4 pos [[position]]; };

vertex VOut vs(uint id [[vertex_id]]) {
    float2 p = float2((id << 1) & 2, id & 2);
    VOut o;
    o.pos = float4(p * 2.0 - 1.0, 0.0, 1.0);
    return o;
}

struct U {
    float2 res;
    float time;
    float voice;
    float wave;
    float twist;
    float writing;
    float scale;
};

float3x3 rot_y(float a) {
    float c = cos(a), s = sin(a);
    return float3x3(float3(c, 0, -s), float3(0, 1, 0), float3(s, 0, c));
}

float3x3 rot_z(float a) {
    float c = cos(a), s = sin(a);
    return float3x3(float3(c, s, 0), float3(-s, c, 0), float3(0, 0, 1));
}

fragment float4 fs(VOut in [[stage_in]], constant U& u [[buffer(0)]]) {
    float2 frame_uv = (in.pos.xy * 2.0 - u.res) / min(u.res.x, u.res.y);
    frame_uv.y = -frame_uv.y;
    float2 uv = frame_uv / max(u.scale, 0.05);

    const float CAM = 2.6;
    const float FOCAL = 1.6;
    float3 ro = float3(0.0, 0.0, CAM);
    float3 rd = normalize(float3(uv, -FOCAL));

    float3 gold = float3(0.85, 0.64, 0.40);
    float3 cream = float3(0.96, 0.91, 0.80);
    float3 lavender = float3(0.62, 0.59, 0.76);

    float3 col = float3(0.0);
    // March only through the unit sphere.
    float b = dot(ro, rd);
    float h = b * b - (dot(ro, ro) - 1.0);
    if (h > 0.0) {
        float sh = sqrt(h);
        float t0 = -b - sh;
        float t1 = -b + sh;
        const int STEPS = 72;
        float dt = (t1 - t0) / float(STEPS);
        float t = t0 + dt * 0.5;
        float3x3 lean = rot_z(0.3);
        for (int i = 0; i < STEPS; i++) {
            float3 p = lean * (ro + rd * t);
            float r = length(p);
            // Shells wound around the axis by their radius; the winding travels outward.
            float3 q = rot_y(u.twist * r * 3.1 - u.wave) * p;
            // Turbulence: the field folds into itself at a few scales.
            float3 w = q * 2.4;
            for (int k = 1; k < 6; k++) {
                float fk = float(k);
                w += sin(w.zxy * fk * 1.1 + u.time * 0.35 * fk) * (0.45 / fk);
            }
            // Thin luminous sheets where a gyroid crosses zero.
            float g = sin(w.x) * cos(w.y) + sin(w.y) * cos(w.z) + sin(w.z) * cos(w.x);
            float sheet = 0.035 / (g * g + 0.035);
            // A soft spine of light along the axis.
            float axis = length(q.xz);
            float spine = exp(-axis * axis * 16.0) * (0.35 + 0.9 * u.voice + 0.6 * u.writing);
            // Dimmer toward the surface, so it reads as a volume and not a shell.
            float body = 1.0 - smoothstep(0.55, 1.0, r);
            float depth = float(i) / float(STEPS);
            float3 tint = mix(gold, cream, depth);
            tint = mix(tint, lavender, smoothstep(0.6, 1.0, r) * (1.0 - u.writing));
            tint = mix(tint, gold, u.writing * 0.5);
            col += tint * (sheet * 3.2 + spine * 1.2) * body * dt;
            t += dt;
        }
    }

    float exposure = 1.4 + 2.4 * u.voice + 1.2 * u.writing;
    float3 inner = 1.0 - exp(-col * exposure);
    // At rest the light goes out: the dot is just a small dark circle.
    float awake = smoothstep(0.14, 0.6, u.scale);
    inner *= awake;

    // A solid black body, like Siri's: the light reads on any background.
    float rs = FOCAL / sqrt(CAM * CAM - 1.0);
    float d = length(uv);
    float aa = 1.5 / (min(u.res.x, u.res.y) * 0.5 * max(u.scale, 0.05));
    float disc = 1.0 - smoothstep(rs - aa, rs + aa, d);
    // A faint cream rim so the edge is defined against dark backgrounds.
    inner += mix(gold, cream, 0.5) * smoothstep(rs * 0.8, rs, d) * disc * (0.12 + 0.2 * u.voice) * max(awake, 0.5);

    // A soft halo outside the body.
    float halo = exp(-max(d - rs, 0.0) * 7.0) * (1.0 - disc) * (0.22 + 0.45 * u.voice + 0.3 * u.writing) * awake;
    float3 halo_col = mix(gold, cream, 0.3) * halo;

    float3 rgb = inner * disc + halo_col;
    float a = disc + (1.0 - disc) * clamp(max(halo_col.r, max(halo_col.g, halo_col.b)) * 1.2, 0.0, 1.0);

    // Never draw to the window edge.
    float fade = 1.0 - smoothstep(0.86, 1.0, length(frame_uv));
    return float4(rgb * fade, a * fade);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Uniforms {
    pub res: [f32; 2],
    pub time: f32,
    pub voice: f32,
    pub wave: f32,
    pub twist: f32,
    pub writing: f32,
    /// Size of the orb in its window: about 0.3 for the idle dot, 1.0 in full.
    pub scale: f32,
}

pub struct Gpu {
    device: Device,
    queue: CommandQueue,
    pipeline: RenderPipelineState,
}

impl Gpu {
    pub fn new() -> Result<Self, String> {
        let device = Device::system_default().ok_or("no Metal device")?;
        let library = device
            .new_library_with_source(SHADER, &CompileOptions::new())
            .map_err(|e| format!("orb shader: {e}"))?;
        let desc = RenderPipelineDescriptor::new();
        let vs = library.get_function("vs", None)?;
        let fs = library.get_function("fs", None)?;
        desc.set_vertex_function(Some(&vs));
        desc.set_fragment_function(Some(&fs));
        desc.color_attachments().object_at(0).ok_or("attachment")?.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        let pipeline = device.new_render_pipeline_state(&desc)?;
        let queue = device.new_command_queue();
        Ok(Self { device, queue, pipeline })
    }

    fn encode(&self, target: &TextureRef, u: &Uniforms) -> &metal::CommandBufferRef {
        let pass = RenderPassDescriptor::new();
        let color = pass.color_attachments().object_at(0).unwrap();
        color.set_texture(Some(target));
        color.set_load_action(MTLLoadAction::Clear);
        color.set_clear_color(MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
        color.set_store_action(MTLStoreAction::Store);
        let buffer = self.queue.new_command_buffer();
        let encoder = buffer.new_render_command_encoder(pass);
        encoder.set_render_pipeline_state(&self.pipeline);
        encoder.set_fragment_bytes(0, std::mem::size_of::<Uniforms>() as u64, u as *const _ as *const _);
        encoder.draw_primitives(MTLPrimitiveType::Triangle, 0, 3);
        encoder.end_encoding();
        buffer
    }

    /// Render one frame offscreen and return BGRA bytes, premultiplied.
    pub fn snapshot(&self, size: u64, u: &Uniforms) -> Vec<u8> {
        let desc = TextureDescriptor::new();
        desc.set_width(size);
        desc.set_height(size);
        desc.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        desc.set_storage_mode(MTLStorageMode::Shared);
        desc.set_usage(MTLTextureUsage::RenderTarget);
        let texture = self.device.new_texture(&desc);
        let buffer = self.encode(&texture, u);
        buffer.commit();
        buffer.wait_until_completed();
        let mut out = vec![0u8; (size * size * 4) as usize];
        texture.get_bytes(
            out.as_mut_ptr() as *mut _,
            size * 4,
            MTLRegion {
                origin: MTLOrigin { x: 0, y: 0, z: 0 },
                size: MTLSize { width: size, height: size, depth: 1 },
            },
            0,
        );
        out
    }
}

/// A borderless, click-through panel at the bottom centre, above the Dock, holding a Metal layer.
pub struct Overlay {
    panel: id,
    /// Centre of the orb in top-left screen points (GPUI's coordinates).
    pub centre: (f64, f64),
    layer: MetalLayer,
    gpu: Gpu,
    pixels: f64,
    visible: bool,
}

const SIZE: f64 = 120.0;

impl Overlay {
    /// Must be called on the main thread.
    pub fn new() -> Result<Self, String> {
        let gpu = Gpu::new()?;
        unsafe {
            let screen = NSScreen::mainScreen(nil);
            let frame = NSScreen::visibleFrame(screen); // excludes the Dock and menu bar
            let scale: f64 = msg_send![screen, backingScaleFactor];
            let rect = NSRect::new(
                NSPoint::new(frame.origin.x + (frame.size.width - SIZE) / 2.0, frame.origin.y + 18.0),
                NSSize::new(SIZE, SIZE),
            );
            let style = NSWindowStyleMask::NSBorderlessWindowMask.bits() | (1 << 7); // non-activating panel
            let panel: id = msg_send![class!(NSPanel), alloc];
            let panel: id = msg_send![panel,
                initWithContentRect: rect
                styleMask: style
                backing: NSBackingStoreType::NSBackingStoreBuffered
                defer: NO];
            let clear: id = msg_send![class!(NSColor), clearColor];
            let _: () = msg_send![panel, setOpaque: NO];
            let _: () = msg_send![panel, setBackgroundColor: clear];
            let _: () = msg_send![panel, setHasShadow: NO];
            let _: () = msg_send![panel, setIgnoresMouseEvents: YES];
            let _: () = msg_send![panel, setReleasedWhenClosed: NO];
            let _: () = msg_send![panel, setLevel: 101i64]; // pop-up level, above normal windows
            // all spaces | stationary | full-screen auxiliary
            let _: () = msg_send![panel, setCollectionBehavior: (1u64 << 0) | (1u64 << 4) | (1u64 << 8)];

            let layer = MetalLayer::new();
            layer.set_device(&gpu.device);
            layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
            layer.set_opaque(false);
            layer.set_framebuffer_only(true);
            layer.set_contents_scale(scale);
            layer.set_drawable_size(CGSize::new(SIZE * scale, SIZE * scale));
            let view: id = msg_send![panel, contentView];
            view.setWantsLayer(YES);
            let _: () = msg_send![view, setLayer: layer.as_ref() as *const _ as id];
            let primary: id = msg_send![class!(NSScreen), screens];
            let primary: id = msg_send![primary, objectAtIndex: 0u64];
            let top = NSScreen::frame(primary).size.height;
            let centre = (rect.origin.x + SIZE / 2.0, top - (rect.origin.y + SIZE / 2.0));
            Ok(Self { panel, centre, layer, gpu, pixels: SIZE * scale, visible: false })
        }
    }

    pub fn show(&mut self) {
        if !self.visible {
            let _: () = unsafe { msg_send![self.panel, orderFrontRegardless] };
            self.visible = true;
        }
    }

    pub fn hide(&mut self) {
        if self.visible {
            let _: () = unsafe { msg_send![self.panel, orderOut: nil] };
            self.visible = false;
        }
    }

    pub fn draw(&self, mut u: Uniforms) {
        if !self.visible {
            return;
        }
        u.res = [self.pixels as f32, self.pixels as f32];
        let Some(drawable) = self.layer.next_drawable() else { return };
        let buffer = self.gpu.encode(drawable.texture(), &u);
        buffer.present_drawable(drawable);
        buffer.commit();
    }
}
