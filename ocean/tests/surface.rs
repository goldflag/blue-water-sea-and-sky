//! Every quality tier builds its field and surface and draws a sea under a gradient sky through the renderer core, as
//! a `SceneLayer` beside the stand-in sky: blue water below a brighter sky, no black holes; and a magnified view's fan
//! covers the sea its lens shows. Needs a GPU adapter.

use glam::Vec3;
use native_offscreen::{CAPTURE_FORMAT, Capture, Gpu};
use naval_ocean::backdrop::StandInSky;
use naval_ocean::environment::{
    CelestialLight, CubeUvLayout, Equirect, StandInEnvironment, gradient_sky,
};
use naval_ocean::{Ocean, QualityTier};
use naval_render::{Camera, FrameInput, FrameTime, RenderOutput, Renderer, SceneLayer};

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;

/// A gradient sky to light the sea and stand behind it.
fn stand_in(gpu: &Gpu) -> StandInEnvironment {
    let sky = gradient_sky(
        256,
        128,
        Vec3::new(0.2, 0.35, 0.7),
        Vec3::new(0.6, 0.65, 0.7),
        Vec3::new(0.01, 0.02, 0.03),
    );
    let light = CelestialLight {
        direction: Vec3::new(0.3, 0.8, -0.5).normalize(),
        color: Vec3::ONE,
        intensity: 6.0,
        night: false,
    };
    StandInEnvironment::new(
        &gpu.device,
        &gpu.queue,
        CubeUvLayout::from_bake_width(256),
        &Equirect {
            width: 256,
            height: 128,
            rgba: &sky,
        },
        &light,
    )
}

#[test]
fn every_tier_draws_a_sea() {
    let gpu = Gpu::headless().expect("a GPU adapter");
    let environment = stand_in(&gpu);
    let mut renderer =
        Renderer::new(&gpu.device, &gpu.queue, WIDTH, HEIGHT, CAPTURE_FORMAT).unwrap();
    let camera_layout = renderer.context().camera_layout.clone();
    let mut backdrop = StandInSky::new(
        &gpu.device,
        &camera_layout,
        &environment.bind_group_layout,
        &environment.bind_group,
    );
    let capture = Capture::new(&gpu, WIDTH, HEIGHT);
    let camera = Camera {
        eye: Vec3::new(0.0, 30.0, 0.0),
        target: Vec3::new(0.0, 0.0, 600.0),
        fov_y_degrees: 55.0,
        near: 0.5,
        far: 60000.0,
        curvature: naval_render::camera::EARTH_CURVATURE,
    };
    for tier in [
        QualityTier::Low,
        QualityTier::Medium,
        QualityTier::High,
        QualityTier::Ultra,
    ] {
        let mut ocean = Ocean::new(
            &gpu.device,
            &gpu.queue,
            tier,
            &camera_layout,
            &environment.bind_group_layout,
            &environment.bind_group,
            60000.0,
        );
        ocean.parameters = naval_ocean::calibration::scene_parameters(
            &naval_ocean::calibration::NORTH_ATLANTIC,
            15.0,
            false,
        );
        for frame in 0..60 {
            let layers: &mut [&mut dyn SceneLayer] = &mut [&mut ocean, &mut backdrop];
            renderer
                .render(
                    FrameInput {
                        camera: &camera,
                        time: FrameTime::fixed(frame, 1.0 / 30.0),
                    },
                    layers,
                    RenderOutput {
                        view: &capture.view,
                        format: CAPTURE_FORMAT,
                        width: WIDTH,
                        height: HEIGHT,
                    },
                )
                .unwrap();
        }
        let pixels = capture.read_rgba(&gpu).unwrap();
        let mean = |rows: std::ops::Range<u32>| {
            let mut sum = [0f64; 3];
            let mut count = 0.0;
            for y in rows {
                for x in 0..WIDTH {
                    let i = ((y * WIDTH + x) * 4) as usize;
                    for (c, total) in sum.iter_mut().enumerate() {
                        *total += f64::from(pixels[i + c]);
                    }
                    count += 1.0;
                }
            }
            sum.map(|s| s / count)
        };
        let sea = mean(HEIGHT * 3 / 4..HEIGHT);
        let sky_colour = mean(0..HEIGHT / 8);
        assert!(sea[2] > sea[0] && sea[2] > 20.0, "{tier:?}: sea {sea:?}");
        assert!(
            sky_colour[2] > sea[2],
            "{tier:?}: sky {sky_colour:?} against sea {sea:?}"
        );
        let black = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .skip((WIDTH * HEIGHT / 2) as usize)
            .filter(|p| p[0] == 0 && p[1] == 0 && p[2] == 0)
            .count();
        assert!(black < 10, "{tier:?}: {black} black pixels in the sea");
    }
}

/// Through binoculars from a foretop the sea is drawn with the fan in the clipmap's place: water in every pixel from
/// the sea's horizon down, whichever way the view turns or tilts, and none where the lens shows only sky.
#[test]
fn a_magnified_view_s_fan_covers_the_sea_in_the_frame() {
    let gpu = Gpu::headless().expect("a GPU adapter");
    let environment = stand_in(&gpu);
    let mut renderer =
        Renderer::new(&gpu.device, &gpu.queue, WIDTH, HEIGHT, CAPTURE_FORMAT).unwrap();
    let camera_layout = renderer.context().camera_layout.clone();
    let capture = Capture::new(&gpu, WIDTH, HEIGHT);
    // High, and Low, whose rows stand deeper than the sea's horizon dips.
    for tier in [QualityTier::High, QualityTier::Low] {
        let mut ocean = Ocean::new(
            &gpu.device,
            &gpu.queue,
            tier,
            &camera_layout,
            &environment.bind_group_layout,
            &environment.bind_group,
            60000.0,
        );
        ocean.parameters = naval_ocean::calibration::scene_parameters(
            &naval_ocean::calibration::NORTH_ATLANTIC,
            15.0,
            false,
        );
        let eye = Vec3::new(40.0, 35.0, -20.0);
        // The lens's magnification and where it looks from the eye.
        let views = [
            (1.0, Vec3::new(0.0, -35.0, 600.0)),
            (8.0, Vec3::new(300.0, -30.0, 10_000.0)),
            (8.0, Vec3::new(-900.0, -35.0, -1200.0)),
            (2.0, Vec3::new(200.0, -35.0, 90.0)),
            (2.0, Vec3::new(-3000.0, 300.0, 500.0)),
            (24.0, Vec3::new(-8000.0, -30.0, 3000.0)),
            (8.0, Vec3::new(0.0, 3000.0, 10_000.0)),
        ];
        for (magnification, toward) in views {
            let tan_half = 26.0_f32.to_radians().tan() / magnification;
            let camera = Camera {
                eye,
                target: eye + toward,
                fov_y_degrees: 2.0 * tan_half.atan().to_degrees(),
                near: 0.5,
                far: 60000.0,
                curvature: naval_render::camera::EARTH_CURVATURE,
            };
            // The sea alone, so a pixel it leaves is the cleared target's black.
            for frame in 0..8 {
                let layers: &mut [&mut dyn SceneLayer] = &mut [&mut ocean];
                renderer
                    .render(
                        FrameInput {
                            camera: &camera,
                            time: FrameTime::fixed(frame, 1.0 / 30.0),
                        },
                        layers,
                        RenderOutput {
                            view: &capture.view,
                            format: CAPTURE_FORMAT,
                            width: WIDTH,
                            height: HEIGHT,
                        },
                    )
                    .unwrap();
            }
            let fan = ocean.surface.fan();
            assert_eq!(
                fan.is_some(),
                magnification > 1.0,
                "{tier:?} {magnification} {toward}"
            );
            // The row the sea's horizon stands on: it dips the root of four times the eye's height times the curvature
            // below level, and the view looks this far above or below level.
            let dip = (4.0 * eye.y * naval_render::camera::EARTH_CURVATURE).sqrt();
            let raised = (toward.y / toward.x.hypot(toward.z)).atan();
            let half = HEIGHT as f32 / 2.0;
            let horizon = half + (raised + dip).tan() / tan_half * half;
            assert_eq!(
                fan.is_none_or(|fan| fan.rows > 0),
                horizon < HEIGHT as f32,
                "{tier:?} {magnification} {toward}"
            );
            let pixels = capture.read_rgba(&gpu).unwrap();
            let water = |x: u32, y: u32| {
                let i = ((y * WIDTH + x) * 4) as usize;
                pixels[i].max(pixels[i + 1]).max(pixels[i + 2]) > 4
            };
            for x in 0..WIDTH {
                let Some(first) = (0..HEIGHT).find(|&y| water(x, y)) else {
                    assert!(
                        horizon > HEIGHT as f32 - 3.0,
                        "{tier:?} {magnification} {toward}: column {x} dry"
                    );
                    continue;
                };
                // A crest may stand a pixel or two over the horizon's row; from the first water down, no gap.
                assert!(
                    (first as f32 - horizon.max(0.0)).abs() < 4.0,
                    "{tier:?} {magnification} {toward}: column {x} from row {first}, the horizon on {horizon}"
                );
                let gap = (first..HEIGHT).find(|&y| !water(x, y));
                assert_eq!(
                    gap, None,
                    "{tier:?} {magnification} {toward}: column {x} from row {first}"
                );
            }
        }
    }
}
