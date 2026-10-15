//! The lakes' water on the GPU (`naval_ocean::lakes`): drawn where a lake's samples are, over land below its level, and
//! nowhere outside them; placed by the land's offset; where the land is drawn coarser than the lakes' grid the lakes'
//! mask draws the outline over land standing a little above the level. Needs a GPU adapter.

use glam::{Vec3, Vec4};
use native_offscreen::{CAPTURE_FORMAT, Capture, Gpu};
use naval_ocean::environment::{
    CelestialLight, CubeUvLayout, Equirect, StandInEnvironment, gradient_sky,
};
use naval_ocean::lakes::{LakeGrid, LakeLayer, LakeQuality, LakeSettings};
use naval_render::{
    Camera, DrawContext, FrameInput, FrameTime, Phase, PipelineId, RenderContext, RenderOutput,
    Renderer, SceneLayer, ScenePipeline,
};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 240;
const LEVEL: f32 = 5.0;

/// Flat land at one height, one colour: what the water stands over.
struct Ground {
    pipeline: PipelineId,
}

impl Ground {
    fn new(ctx: &mut RenderContext<'_>, height: f32, tag: &str) -> Self {
        let path = format!("tests/lake-ground-{tag}.wgsl");
        ctx.library
            .add_source(
                path.clone(),
                format!(
                    "#import naval::camera::world_to_clip\n\
                     @vertex fn vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{\n\
                       var corners = array<vec2<f32>, 6>(vec2(-1.0, -1.0), vec2(-1.0, 1.0), vec2(1.0, 1.0), \
                       vec2(-1.0, -1.0), vec2(1.0, 1.0), vec2(1.0, -1.0));\n\
                       let c = corners[i] * 20000.0;\n\
                       return world_to_clip(vec3<f32>(c.x, {height:.3}, c.y));\n\
                     }}\n\
                     @fragment fn fragment() -> @location(0) vec4<f32> {{ return vec4<f32>(0.12, 0.10, 0.06, 1.0); }}\n"
                ),
            )
            .unwrap();
        let pipeline = ctx
            .add_scene_pipeline(
                "lake test ground",
                &path,
                ScenePipeline::new("vertex", "fragment"),
            )
            .unwrap();
        Self { pipeline }
    }
}

impl SceneLayer for Ground {
    fn label(&self) -> &str {
        "ground"
    }
    fn draw(&self, phase: Phase, pass: &mut wgpu::RenderPass<'_>, ctx: &DrawContext<'_>) {
        if phase == Phase::Opaque {
            pass.set_pipeline(ctx.pipelines.render(self.pipeline));
            pass.draw(0..6, 0..1);
        }
    }
}

/// A round lake of radius 5 samples about sample (10, 10) of a 21 × 21 grid of 70 m samples centred on the chart's
/// origin, at [`LEVEL`].
fn round_lake() -> LakeGrid {
    let mut levels = vec![f32::NAN; 21 * 21];
    for r in 0..21 {
        for c in 0..21 {
            let (dc, dr) = (c as f32 - 10.0, r as f32 - 10.0);
            if dc * dc + dr * dr <= 25.0 {
                levels[r * 21 + c] = LEVEL;
            }
        }
    }
    LakeGrid {
        origin: [-700.0, -700.0],
        cell: 70.0,
        columns: 21,
        rows: 21,
        levels,
    }
}

struct Bench {
    gpu: Gpu,
    environment: StandInEnvironment,
    capture: Capture,
    camera: Camera,
}

impl Bench {
    fn new() -> Self {
        let gpu = Gpu::headless().expect("a GPU adapter");
        let sky = gradient_sky(
            256,
            128,
            Vec3::new(0.25, 0.4, 0.75),
            Vec3::new(0.65, 0.7, 0.75),
            Vec3::new(0.05, 0.05, 0.05),
        );
        let light = CelestialLight {
            direction: Vec3::new(0.3, 0.8, -0.5).normalize(),
            color: Vec3::ONE,
            intensity: 6.0,
            night: false,
        };
        let environment = StandInEnvironment::new(
            &gpu.device,
            &gpu.queue,
            CubeUvLayout::from_bake_width(256),
            &Equirect {
                width: 256,
                height: 128,
                rgba: &sky,
            },
            &light,
        );
        let capture = Capture::new(&gpu, WIDTH, HEIGHT);
        // From 1.2 km up, 600 m south, looking down at the lake: flat (the test's points project without the curve).
        let camera = Camera {
            eye: Vec3::new(0.0, 1200.0, 600.0),
            target: Vec3::new(0.0, 0.0, 0.0),
            fov_y_degrees: 52.0,
            near: 1.0,
            far: 60_000.0,
            curvature: 0.0,
        };
        Self {
            gpu,
            environment,
            capture,
            camera,
        }
    }

    /// The frame with the ground at `ground` m, and the lakes with `settings` placed at `offset` (none without).
    fn render(&self, ground: f32, lakes: Option<(LakeSettings, [f64; 2])>) -> Vec<u8> {
        let mut renderer = Renderer::new(
            &self.gpu.device,
            &self.gpu.queue,
            WIDTH,
            HEIGHT,
            CAPTURE_FORMAT,
        )
        .unwrap();
        let tag = format!("{}", (ground * 10.0) as i32);
        let mut land = Ground::new(&mut renderer.context(), ground, &tag);
        let mut water = lakes.map(|(settings, offset)| {
            let mut layer = LakeLayer::new(
                &mut renderer.context(),
                &round_lake(),
                settings,
                &self.environment.bind_group_layout,
                &self.environment.bind_group,
            )
            .unwrap();
            layer.set_offset(offset);
            layer.set_fog(false);
            layer
        });
        for frame in 0..3 {
            let mut layers: Vec<&mut dyn SceneLayer> = vec![&mut land];
            if let Some(water) = water.as_mut() {
                layers.push(water);
            }
            renderer
                .render(
                    FrameInput {
                        camera: &self.camera,
                        time: FrameTime::fixed(frame, 1.0 / 30.0),
                    },
                    &mut layers,
                    RenderOutput {
                        view: &self.capture.view,
                        format: CAPTURE_FORMAT,
                        width: WIDTH,
                        height: HEIGHT,
                    },
                )
                .unwrap();
        }
        if let Some(water) = &water {
            assert!(water.counts().drawn_chunks > 0, "the lake is in view");
        }
        self.capture.read_rgba(&self.gpu).unwrap()
    }

    /// The pixel a world point lands on.
    fn pixel(&self, world: Vec3) -> (u32, u32) {
        let vp = self.camera.projection(WIDTH as f32 / HEIGHT as f32) * self.camera.view();
        let clip = vp * Vec4::new(world.x, world.y, world.z, 1.0);
        let ndc = clip.truncate() / clip.w;
        let x = ((ndc.x + 1.0) / 2.0 * WIDTH as f32) as u32;
        let y = ((1.0 - ndc.y) / 2.0 * HEIGHT as f32) as u32;
        (x, y)
    }
}

/// How far apart two frames are at a world point (the largest channel difference over the pixel and its neighbours).
fn differs(bench: &Bench, a: &[u8], b: &[u8], world: Vec3) -> u8 {
    let (x, y) = bench.pixel(world);
    let mut most = 0;
    for dy in -1i32..=1 {
        for dx in -1i32..=1 {
            let (px, py) = (x as i32 + dx, y as i32 + dy);
            assert!(
                px >= 0 && py >= 0 && px < WIDTH as i32 && py < HEIGHT as i32,
                "{world} in view"
            );
            let i = ((py as u32 * WIDTH + px as u32) * 4) as usize;
            for c in 0..3 {
                most = most.max(a[i + c].abs_diff(b[i + c]));
            }
        }
    }
    most
}

#[test]
fn a_lake_draws_water_where_its_samples_are_and_nothing_outside_them() {
    let bench = Bench::new();
    let near = LakeSettings {
        quality: LakeQuality::High,
        land_cell: 40.0,
        land_spacing: 20.0,
        land_pixels: 5.0,
    };
    // The land a metre under the level everywhere: the water shows wherever the lake's mesh and mask let it.
    let ground = LEVEL - 1.0;
    let bare = bench.render(ground, None);
    let wet = bench.render(ground, Some((near, [0.0, 0.0])));
    let at = |x: f32, z: f32| Vec3::new(x, ground, z);
    // Inside: the lake's samples, out to the last ring of them (5 samples, 350 m, from its middle).
    for (x, z) in [
        (0.0, 0.0),
        (210.0, 0.0),
        (-280.0, 70.0),
        (0.0, -350.0),
        (0.0, 350.0),
        (-140.0, -210.0),
    ] {
        let d = differs(&bench, &bare, &wet, at(x, z));
        assert!(d > 12, "water at ({x}, {z}): {d}");
    }
    // Outside: two samples beyond the last lake sample, and the corners of the grid.
    for (x, z) in [
        (500.0, 0.0),
        (-500.0, 0.0),
        (0.0, 500.0),
        (0.0, -500.0),
        (420.0, -420.0),
        (-550.0, -550.0),
    ] {
        let d = differs(&bench, &bare, &wet, at(x, z));
        assert!(d <= 1, "no water at ({x}, {z}): {d}");
    }
    let image = image::RgbaImage::from_raw(WIDTH, HEIGHT, wet.clone()).unwrap();
    let out = native_offscreen::build_dir("ocean");
    std::fs::create_dir_all(&out).unwrap();
    image.save(out.join("lake-gpu-test.png")).unwrap();

    // Placed 280 m east (the land's offset), the water goes with it.
    let moved = bench.render(ground, Some((near, [280.0, 0.0])));
    assert!(
        differs(&bench, &bare, &moved, at(560.0, 0.0)) > 12,
        "moved east"
    );
    assert!(
        differs(&bench, &bare, &moved, at(-420.0, 0.0)) <= 1,
        "gone from the west shore"
    );

    // Land a metre above the level: drawn finely, it hides the water; where the land is coarser than the lakes' grid
    // (an air window's 140 m samples) the mask draws the lake over it.
    let high = LEVEL + 1.0;
    let dry = bench.render(high, None);
    let hidden = bench.render(high, Some((near, [0.0, 0.0])));
    let at = |x: f32, z: f32| Vec3::new(x, high, z);
    for (x, z) in [(0.0, 0.0), (210.0, 0.0)] {
        assert!(
            differs(&bench, &dry, &hidden, at(x, z)) <= 1,
            "the land hides the lake"
        );
    }
    let coarse = LakeSettings {
        land_cell: 140.0,
        ..near
    };
    let led = bench.render(high, Some((coarse, [0.0, 0.0])));
    for (x, z) in [(0.0, 0.0), (210.0, 0.0), (0.0, -280.0)] {
        assert!(
            differs(&bench, &dry, &led, at(x, z)) > 12,
            "the mask draws the lake at ({x}, {z})"
        );
    }
    for (x, z) in [(500.0, 0.0), (0.0, 500.0), (420.0, -420.0)] {
        assert!(
            differs(&bench, &dry, &led, at(x, z)) <= 1,
            "nothing beyond the mask at ({x}, {z})"
        );
    }
    // Medium draws the lake too, by its cheaper path.
    let medium = LakeSettings {
        quality: LakeQuality::Medium,
        ..near
    };
    let cheap = bench.render(ground, Some((medium, [0.0, 0.0])));
    let at = |x: f32, z: f32| Vec3::new(x, ground, z);
    assert!(
        differs(&bench, &bare, &cheap, at(0.0, 0.0)) > 12,
        "Medium's water"
    );
    assert!(
        differs(&bench, &bare, &cheap, at(500.0, 0.0)) <= 1,
        "Medium: none outside"
    );

    // Over the sea's floor (land under the sea's level, which no lake's bed is) no lake is drawn, though its samples
    // say there is one: a lagoon's squares reaching over the sea beside it.
    let seabed = -2.0;
    let sea = bench.render(seabed, None);
    let over = bench.render(seabed, Some((near, [0.0, 0.0])));
    let at = |x: f32, z: f32| Vec3::new(x, seabed, z);
    for (x, z) in [(0.0, 0.0), (210.0, 0.0)] {
        assert!(
            differs(&bench, &sea, &over, at(x, z)) <= 1,
            "no lake over the sea's floor at ({x}, {z})"
        );
    }
}
