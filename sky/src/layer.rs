//! The sky as a renderer-core layer (`naval_render::SceneLayer`, `native/docs/render-core.md`): the tables and
//! the environment in `prepare` on the frame's encoder, the dome in the `Sky` phase. The sky's own bindings are
//! at group 1, so the camera the renderer binds at group 0 stays put; the dome takes its view rays from the
//! camera's matrices, copied into the sky's uniforms.
//!
//! Not yet through the renderer's pipeline registry: the sky composes and owns its pipelines, so its shaders
//! do not hot-reload in the harness (see `native/docs/sky.md`, open problems).

use glam::{Mat4, Vec3};
use naval_render::{
    CameraUniforms, DEPTH_FORMAT, DrawContext, HDR_FORMAT, Phase, PrepareContext, SceneLayer,
};

use crate::sky::{Sky, SkyQuality, SkyTargets, SkyView};

/// The sky's view from the renderer core's camera block.
pub fn sky_view(camera: &CameraUniforms, viewport: (u32, u32)) -> SkyView {
    SkyView {
        position: Vec3::from(camera.position),
        camera_to_world: Mat4::from_cols_array(&camera.inverse_view),
        projection: Mat4::from_cols_array(&camera.projection),
        viewport: [viewport.0, viewport.1],
    }
}

/// A [`Sky`] drawn by the renderer core. Reach the sky itself (to apply a scene, or for its environment and
/// light) through `sky`.
pub struct SkyLayer {
    pub sky: Sky,
}

impl SkyLayer {
    /// A sky whose dome fits the renderer core's scene pass (`HDR_FORMAT`, reversed `DEPTH_FORMAT`).
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, quality: SkyQuality) -> Self {
        let targets = SkyTargets {
            color: HDR_FORMAT,
            depth: Some(DEPTH_FORMAT),
        };
        Self {
            sky: Sky::new(device, queue, quality, targets),
        }
    }
}

impl SceneLayer for SkyLayer {
    fn label(&self) -> &str {
        "sky"
    }

    fn prepare(&mut self, ctx: &mut PrepareContext<'_>) {
        let view = sky_view(ctx.camera, ctx.viewport);
        // The renderer's clock is absolute (frame N is at N x dt), so a frame rendered twice shows the same sky.
        self.sky.seek(ctx.time.seconds);
        self.sky.update(ctx.queue, ctx.encoder, &view, 0.0);
    }

    fn draw(&self, phase: Phase, pass: &mut wgpu::RenderPass<'_>, _ctx: &DrawContext<'_>) {
        if phase == Phase::Sky {
            self.sky.draw_dome(pass);
        }
    }
}
