//! The sky drawn by the renderer core (`naval_render::Renderer`) as a `SceneLayer`, through its scene pass
//! and display transform (native only):
//!
//!     cargo run -p naval-sky --example render_core --release
//!
//! Writes `.build/native/sky/render-core/dome-<elevation>-<facing>.png`, the same views as the stills
//! example's `dome-*` images, so the two paths can be compared.

use anyhow::Result;
use glam::Vec3;
use native_offscreen::{CAPTURE_FORMAT, Capture, Gpu};
use naval_render::{Camera, FrameInput, FrameTime, RenderOutput, Renderer};
use naval_sky::{SkyLayer, SkyQuality, SkyScene};

fn main() -> Result<()> {
    let gpu = Gpu::headless()?;
    let out = native_offscreen::build_dir("sky").join("render-core");
    std::fs::create_dir_all(&out)?;
    let (width, height) = (960, 540);
    let mut renderer = Renderer::new(&gpu.device, &gpu.queue, width, height, CAPTURE_FORMAT)?;
    let mut layer = SkyLayer::new(&gpu.device, &gpu.queue, SkyQuality::High);
    let capture = Capture::new(&gpu, width, height);
    for (elevation, facing, turn) in [
        (20.0, "sun", 0.0f32),
        (5.0, "sun", 0.0),
        (-6.0, "away", 180.0),
    ] {
        layer.sky.apply(&SkyScene::north_atlantic(elevation, 270.0));
        let sun = layer.sky.sun_direction();
        let azimuth = sun.x.atan2(sun.z) + turn.to_radians();
        let pitch = 8f32.to_radians();
        let eye = Vec3::new(0.0, 30.0, 0.0);
        let forward = Vec3::new(
            azimuth.sin() * pitch.cos(),
            pitch.sin(),
            azimuth.cos() * pitch.cos(),
        );
        let camera = Camera::look_at(eye, eye + forward);
        let mut layers: [&mut dyn naval_render::SceneLayer; 1] = [&mut layer];
        renderer.render(
            FrameInput {
                camera: &camera,
                time: FrameTime::fixed(0, 1.0 / 60.0),
            },
            &mut layers,
            RenderOutput {
                view: &capture.view,
                format: CAPTURE_FORMAT,
                width,
                height,
            },
        )?;
        let path = out.join(format!("dome-{elevation}-{facing}.png"));
        capture.save_png(&gpu, &path)?;
        println!("{}", path.display());
    }
    Ok(())
}
