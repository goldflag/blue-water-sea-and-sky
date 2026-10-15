//! The wake field on the GPU, checked as `scripts/diagnostics/ocean-wake.html` checks the web's: a Bismarck-sized hull's
//! bow and stern generators driven as `ShipWake` drives them, the state read back.

use native_offscreen::Gpu;
use naval_ocean::ship_wake::BREAK_REACH;
use naval_ocean::wake::WakeField;
use naval_ocean::wake::generators::GeneratorOptions;

const DT: f64 = 1.0 / 60.0;
const MAX_SPEED: f64 = 15.0;
const LENGTH: f64 = 250.0;
const BEAM: f64 = 36.0;

struct Hull {
    x: f64,
    z: f64,
    heading: f64,
    speed: f64,
    turn: f64,
    bow: u32,
    stern: u32,
}

/// One game frame: move the hull (bow toward −Z at heading 0), drive the generators, step the field.
fn frame(gpu: &Gpu, field: &mut WakeField, hull: &mut Hull, dt: f64) {
    hull.heading += hull.turn * dt;
    hull.x += hull.heading.sin() * hull.speed * dt;
    hull.z -= hull.heading.cos() * hull.speed * dt;
    let ratio = (hull.speed.abs() / MAX_SPEED).min(1.0);
    let active = hull.speed > 0.1;
    field.generators.update(hull.bow, |g| {
        g.active = active;
        g.depth = 0.32 * ratio * ratio;
    });
    field.generators.update(hull.stern, |g| {
        g.active = active;
        g.depth = 0.18 * ratio * ratio;
    });
    for id in [hull.bow, hull.stern] {
        field.generators.set_pose(id, hull.x, hull.z, hull.heading);
    }
    field.foam_strength = 1.2 * ratio;
    field.set_center(hull.x, hull.z);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    field.update(&gpu.queue, &mut encoder, dt);
    gpu.queue.submit([encoder.finish()]);
}

fn run(gpu: &Gpu, field: &mut WakeField, hull: &mut Hull, seconds: f64) {
    for _ in 0..(seconds / DT).round() as usize {
        frame(gpu, field, hull, DT);
    }
}

/// The newest state, RGBA f32 per texel, rows top first.
fn read_state(gpu: &Gpu, field: &WakeField) -> Vec<f32> {
    let n = field.resolution();
    let row = n * 16;
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * n),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        field.state_texture().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(n),
            },
        },
        wgpu::Extent3d {
            width: n,
            height: n,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| result.unwrap());
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    bytemuck::cast_slice::<u8, f32>(&slice.get_mapped_range().unwrap()).to_vec()
}

struct State {
    data: Vec<f32>,
    n: usize,
    origin: [f64; 2],
    cell: f64,
}

impl State {
    fn read(gpu: &Gpu, field: &WakeField) -> Self {
        Self {
            data: read_state(gpu, field),
            n: field.resolution() as usize,
            origin: field.origin(),
            cell: field.world_size() / f64::from(field.resolution()),
        }
    }

    fn at(&self, x: f64, z: f64) -> [f32; 4] {
        let i = ((x - self.origin[0]) / self.cell + self.n as f64 / 2.0).floor();
        let j = ((z - self.origin[1]) / self.cell + self.n as f64 / 2.0).floor();
        if i < 0.0 || j < 0.0 || i >= self.n as f64 || j >= self.n as f64 {
            return [0.0; 4];
        }
        let index = (j as usize * self.n + i as usize) * 4;
        [
            self.data[index],
            self.data[index + 1],
            self.data[index + 2],
            self.data[index + 3],
        ]
    }

    fn max_height(&self) -> f32 {
        self.data.chunks(4).map(|t| t[0].abs()).fold(0.0, f32::max)
    }

    fn max_foam(&self) -> f32 {
        self.data.chunks(4).map(|t| t[2]).fold(0.0, f32::max)
    }

    fn total_foam(&self) -> f64 {
        self.data.chunks(4).map(|t| f64::from(t[2])).sum()
    }

    fn finite(&self) -> bool {
        self.data.iter().all(|v| v.is_finite())
    }
}

/// A point `behind` metres astern of the stern generator.
fn stern_point(hull: &Hull, behind: f64) -> (f64, f64) {
    let back = LENGTH * 0.448 + behind;
    (
        hull.x - hull.heading.sin() * back,
        hull.z + hull.heading.cos() * back,
    )
}

/// Full width at half maximum (m) of the foam across the track at a point.
fn foam_width(state: &State, hull: &Hull, at: (f64, f64)) -> f64 {
    let across = (hull.heading.cos(), hull.heading.sin());
    let profile: Vec<f32> = (-300..=300)
        .map(|s| {
            let s = f64::from(s) * 0.5;
            state.at(at.0 + across.0 * s, at.1 + across.1 * s)[2]
        })
        .collect();
    let peak = profile.iter().copied().fold(0.0, f32::max);
    profile.iter().filter(|&&v| v > peak / 2.0).count() as f64 * 0.5
}

#[test]
fn the_wake_field_behaves_like_the_webs() {
    let gpu = Gpu::headless().expect("a GPU");
    let mut field = WakeField::new(&gpu.device, 512, 1536.0);
    let options = |depth: f64, radius: f64, offset: f64| GeneratorOptions {
        active: false,
        depth,
        radius,
        teleport_threshold: 100.0,
        offset: [0.0, offset],
        breaks_to: [0.0, 0.0],
        break_reach: BEAM * BREAK_REACH,
    };
    let bow = field
        .generators
        .add(options(0.32, BEAM * 0.28, -LENGTH * 0.448))
        .unwrap();
    let stern = field
        .generators
        .add(options(0.18, BEAM * 0.39, LENGTH * 0.448))
        .unwrap();
    let mut hull = Hull {
        x: 0.0,
        z: 0.0,
        heading: 0.0,
        speed: 0.0,
        turn: 0.0,
        bow,
        stern,
    };

    // The web's figures for the same run (`bun scripts/browser/ocean-wake.ts --resolution 512`, 2026-09-30, Chromium on
    // this Apple M5 Pro): its probe reads the sampler on a 512² grid over the field's 1,536 m, the same cells as the
    // state read back here.
    let near = |ours: f64, web: f64, share: f64| (ours - web).abs() <= web.abs() * share;

    run(&gpu, &mut field, &mut hull, 1.0);
    let rest = State::read(&gpu, &field);
    assert_eq!(rest.max_height(), 0.0, "a hull at rest leaves the sea calm");
    assert_eq!(rest.max_foam(), 0.0);

    // Straight at 15 m/s for 30 s.
    hull.speed = MAX_SPEED;
    run(&gpu, &mut field, &mut hull, 30.0);
    let straight = State::read(&gpu, &field);
    let (tx, tz) = stern_point(&hull, 150.0);
    let trail = f64::from(straight.at(tx, tz)[2]);
    let max_height = f64::from(straight.max_height());
    let straight_foam = straight.total_foam();
    eprintln!("straight: trail foam {trail}, max height {max_height}, total foam {straight_foam}");
    assert!(
        near(trail, 0.0802, 0.08),
        "trail foam 150 m astern {trail}, web 0.0802"
    );
    assert!(
        near(max_height, 0.303955, 0.02),
        "max height {max_height}, web 0.3040"
    );
    assert!(
        near(straight_foam, 155.911, 0.03),
        "total foam {straight_foam}, web 155.9"
    );
    assert!(straight.finite() && max_height < 7.99);
    let fresh = foam_width(&straight, &hull, stern_point(&hull, 150.0));
    let older = foam_width(&straight, &hull, stern_point(&hull, 300.0));
    eprintln!("foam widths: {fresh} m then {older} m (web 23.5, 31.5)");
    assert!(older > fresh, "the wake spreads: {fresh} m then {older} m");
    assert!(near(fresh, 23.5, 0.25) && near(older, 31.5, 0.25));
    // The field ahead of the hull is calm; waves reach out abeam of the trail (the Kelvin wedge).
    let ahead = straight.at(
        hull.x + hull.heading.sin() * 400.0,
        hull.z - hull.heading.cos() * 400.0,
    );
    assert!(ahead[0].abs() < 0.01, "calm ahead: {}", ahead[0]);
    let abeam = (0..40)
        .map(|k| straight.at(tx + f64::from(k) * 3.0 + 40.0, tz)[0].abs())
        .fold(0.0, f32::max);
    assert!(abeam > 0.005, "waves abeam of the trail: {abeam}");

    // A paused frame changes nothing.
    for _ in 0..8 {
        frame(&gpu, &mut field, &mut hull, 0.0);
    }
    let paused = State::read(&gpu, &field);
    assert!(paused.data == straight.data, "pause preserves the wake");

    // Turning for 20 s (the web turns its yaw up 0.02 rad/s: the heading down): the old track keeps its foam.
    hull.turn = -0.02;
    run(&gpu, &mut field, &mut hull, 20.0);
    let turned = State::read(&gpu, &field);
    let old_track = f64::from(turned.at(tx, tz)[2]);
    eprintln!("old track foam {old_track} (web 0.00602)");
    assert!(old_track > 0.005 && near(old_track, 0.00602, 0.3));

    // Stopping fades the foam.
    let before_stop = turned.total_foam();
    hull.turn = 0.0;
    hull.speed = 0.0;
    run(&gpu, &mut field, &mut hull, 20.0);
    let stopped = State::read(&gpu, &field).total_foam();
    eprintln!("foam after stop {stopped} (web 17.42)");
    assert!(stopped < before_stop * 0.5 && near(stopped, 17.417, 0.1));

    // Reset clears everything.
    field.reset(&gpu.device);
    let cleared = State::read(&gpu, &field);
    assert!(cleared.data.iter().all(|&v| v == 0.0), "reset clears");

    // A teleport leaves no streak.
    hull.speed = MAX_SPEED;
    run(&gpu, &mut field, &mut hull, 3.0);
    let from = (hull.x, hull.z);
    hull.x += 600.0;
    run(&gpu, &mut field, &mut hull, 3.0);
    let across = State::read(&gpu, &field);
    let middle = across.at(from.0 + 300.0, from.1);
    assert!(
        middle[2] < 0.01 && middle[0].abs() < 0.01,
        "teleport streak: {middle:?}"
    );

    // Breaking: the straight run again with the game's low break threshold, where only breaking can add foam.
    field.reset(&gpu.device);
    hull.heading = 0.0;
    field.foam_break_threshold = 0.015;
    run(&gpu, &mut field, &mut hull, 30.0);
    let breaking = State::read(&gpu, &field).total_foam();
    eprintln!("breaking foam {breaking} (web 578.26)");
    assert!(breaking > 1.2 * straight_foam && near(breaking, 578.26, 0.05));
}

/// What strikes the sea rings it as deep water rings: a crater's waves, read along a line from it, have the local
/// wavelength of the Cauchy–Poisson problem's stationary phase (k = g t² / 4r², the longest waves furthest out), with a
/// hull's breaking threshold set and none of them foaming; and the field steps nothing it was not given.
#[test]
fn a_crater_rings_the_sea_with_deep_water_dispersion() {
    use naval_ocean::wake::impulses::{Strike, WakeImpulse};
    let gpu = Gpu::headless().expect("a GPU");
    let mut field = WakeField::new(&gpu.device, 512, 1536.0);
    field.foam_strength = 1.2;
    field.foam_break_threshold = 0.015;
    let step = |field: &mut WakeField, seconds: f64| {
        for _ in 0..(seconds / DT).round() as usize {
            let mut encoder = gpu.device.create_command_encoder(&Default::default());
            field.update(&gpu.queue, &mut encoder, DT);
            gpu.queue.submit([encoder.finish()]);
        }
    };
    step(&mut field, 0.5);
    assert_eq!(
        State::read(&gpu, &field).max_height(),
        0.0,
        "a calm field stays calm"
    );
    // Off the field: nothing laid.
    assert!(!field.strike(
        5000.0,
        0.0,
        &Strike::Charge {
            kg: 70.0,
            depth: 0.0
        }
    ));
    // A 6 m crater at the centre, laid at once.
    assert!(field.add_impulse(WakeImpulse {
        displacement: -2.0,
        foam: 1.0,
        ..WakeImpulse::at(0.0, 0.0, 6.0)
    }));
    step(&mut field, 2.0 * DT);
    let laid = State::read(&gpu, &field);
    let bottom = laid.at(0.0, 0.0)[0];
    assert!(
        (-2.2..-1.5).contains(&bottom),
        "the crater's floor {bottom}"
    );
    assert!(laid.at(0.0, 0.0)[2] > 0.7, "white water over it");
    let seconds = 25.0;
    step(&mut field, seconds - 2.0 * DT);
    let state = State::read(&gpu, &field);
    assert!(state.finite());
    // Zero crossings along +x: each half wavelength against 8π r² / (g t²) at its middle.
    let profile: Vec<(f64, f32)> = (0..1600)
        .map(|i| {
            let r = 20.0 + f64::from(i) * 0.25;
            (r, state.at(r, 0.0)[0])
        })
        .collect();
    let crossings: Vec<f64> = profile
        .windows(2)
        .filter(|w| (w[0].1 > 0.0) != (w[1].1 > 0.0))
        .map(|w| w[0].0)
        .collect();
    let mut checked = 0;
    for pair in crossings.windows(2) {
        let middle = 0.5 * (pair[0] + pair[1]);
        let theory = 8.0 * std::f64::consts::PI * middle * middle / (9.81 * seconds * seconds);
        // Only waves the grid carries (6 cells of 3 m and up) and the train has reached.
        if !(24.0..120.0).contains(&theory) {
            continue;
        }
        let measured = 2.0 * (pair[1] - pair[0]);
        eprintln!("at {middle:.0} m: {measured:.1} m, deep water {theory:.1} m");
        assert!(
            (measured / theory - 1.0).abs() < 0.25,
            "at {middle:.0} m: a {measured:.1} m wave where deep water has {theory:.1} m"
        );
        checked += 1;
    }
    assert!(checked >= 3, "{checked} half waves checked: {crossings:?}");
    let ring = profile
        .iter()
        .filter(|(r, _)| (60.0..160.0).contains(r))
        .map(|(_, h)| h.abs())
        .fold(0.0, f32::max);
    eprintln!("ring height {ring} m");
    assert!((0.01..0.5).contains(&ring), "ring height {ring}");
    let foam = (60..160)
        .map(|r| state.at(f64::from(r), 0.0)[2])
        .fold(0.0, f32::max);
    assert!(foam < 0.02, "foam on the rings {foam}");
}

/// A salvo's rings never break while a hull steams by: on Ultra's grid (the finest, whose rings are the steepest) a
/// 38 cm round and a 16-inch HE burster ring the sea for a minute, long after their trains are forgotten, with a
/// battleship at full speed 400 m off setting the hulls' breaking threshold. The white water laid over the craters
/// fades and the rings never add to it, while her track keeps its own foam (how much of it her breaking is,
/// `the_wake_field_behaves_like_the_webs` checks).
#[test]
fn a_salvos_rings_never_break_beside_a_hull() {
    use naval_ocean::wake::impulses::Strike;
    let gpu = Gpu::headless().expect("a GPU");
    let mut field = WakeField::new(&gpu.device, 1024, 1536.0);
    field.foam_break_threshold = 0.015;
    field.set_center(0.0, 0.0);
    let (length, beam, speed) = (250.0, 36.0, 15.0);
    let options = |depth: f64, radius: f64, offset: f64| GeneratorOptions {
        active: true,
        depth,
        radius,
        teleport_threshold: 100.0,
        offset: [0.0, offset],
        breaks_to: [0.0, 0.0],
        break_reach: beam * BREAK_REACH,
    };
    let bow = field
        .generators
        .add(options(0.32, beam * 0.28, -length * 0.448))
        .unwrap();
    let stern = field
        .generators
        .add(options(0.18, beam * 0.39, length * 0.448))
        .unwrap();
    // Across the field 400 m off the falls, heading +X (heading π/2: bow toward +X).
    let mut x = -500.0;
    let z = -400.0;
    let mut frame = |field: &mut WakeField| {
        x += speed * DT;
        for id in [bow, stern] {
            field
                .generators
                .set_pose(id, x, z, std::f64::consts::FRAC_PI_2);
        }
        field.foam_strength = 1.2;
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        field.update(&gpu.queue, &mut encoder, DT);
        gpu.queue.submit([encoder.finish()]);
    };
    for _ in 0..(3.0 / DT) as usize {
        frame(&mut field);
    }
    assert!(field.strike(
        0.0,
        0.0,
        &Strike::Round {
            caliber_m: 0.38,
            speed: 400.0
        }
    ));
    assert!(field.strike(
        150.0,
        100.0,
        &Strike::Charge {
            kg: 70.0,
            depth: 0.0
        }
    ));
    let mut previous = f32::INFINITY;
    for second in 1..=60 {
        for _ in 0..(1.0 / DT).round() as usize {
            frame(&mut field);
        }
        if second % 5 != 0 {
            continue;
        }
        let state = State::read(&gpu, &field);
        assert!(state.finite());
        // The rings about both falls, on the side away from her track, out from the middle of each crater.
        let mut foam: f32 = 0.0;
        for (cx, cz) in [(0.0, 0.0), (150.0, 100.0)] {
            for r in (30..300).step_by(2) {
                for a in 0..36 {
                    let angle = f64::from(a) * std::f64::consts::TAU / 36.0;
                    let (px, pz) = (
                        cx + f64::from(r) * angle.cos(),
                        cz + f64::from(r) * angle.sin(),
                    );
                    if pz > -150.0 {
                        foam = foam.max(state.at(px, pz)[2]);
                    }
                }
            }
        }
        eprintln!("{second} s: foam on the rings {foam}");
        // The white water each strike laid over its crater only fades and spreads: the rings never add to it.
        assert!(
            foam <= previous,
            "the rings broke at {second} s: foam {foam}, {previous} before"
        );
        assert!(
            second < 25 || foam < 0.02,
            "foam on the rings at {second} s: {foam}"
        );
        previous = foam;
    }
    // Her track keeps its foam.
    let state = State::read(&gpu, &field);
    let track = (0..200)
        .map(|k| state.at(x - 60.0 - f64::from(k) * 2.0, z)[2])
        .fold(0.0, f32::max);
    eprintln!("foam along her track {track}");
    assert!(track > 0.1, "her track's foam {track}");
}
