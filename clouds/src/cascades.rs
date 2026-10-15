//! The Beer shadow maps' cascades (`CascadedShadowMaps.ts`, `helpers/FrustumCorners.ts` and `helpers/splitFrustum.ts`
//! of @takram/three-clouds, after three.js's CSM and StrandedKitty's three-csm, MIT): the camera's frustum, out to the
//! cascades' far, split between a uniform and a logarithmic division (`practical`, λ 0.6), each slice enclosed by a
//! square orthographic view from the sun, its centre snapped to whole texels in the light's frame so the maps do not
//! swim as the camera turns, and the light camera stood back along the sunlight.

use glam::{Mat4, Vec2, Vec3, Vec4};

/// The most cascades the shaders hold.
pub const MAX_CASCADES: usize = 4;

/// One cascade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cascade {
    /// The slice of the view, in fractions of the cascades' far (`viewZToOrthographicDepth`).
    pub interval: [f32; 2],
    /// World → the cascade's clip.
    pub matrix: Mat4,
    /// The cascade's clip → world.
    pub inverse_matrix: Mat4,
    /// The world corners of the slice of the view this cascade may serve: its interval widened by the bands in which
    /// the shader's faded cascade index can still pick it (`getFadedCascadeIndex`: a quarter of the edge's square on
    /// each side). A cascade marched for an earlier view still serves this one while every corner lies in its square.
    pub reach: [Vec3; 8],
}

impl Default for Cascade {
    fn default() -> Self {
        Self {
            interval: [0.0; 2],
            matrix: Mat4::IDENTITY,
            inverse_matrix: Mat4::IDENTITY,
            reach: [Vec3::ZERO; 8],
        }
    }
}

impl Cascade {
    /// Whether a world point lies within this cascade's square.
    pub fn covers_point(&self, point: Vec3) -> bool {
        let clip = self.matrix * point.extend(1.0);
        clip.x.abs() <= clip.w && clip.y.abs() <= clip.w
    }

    /// Whether every corner of `reach` (another fit's slice) lies within this cascade's square.
    pub fn covers(&self, reach: &[Vec3; 8]) -> bool {
        reach.iter().all(|corner| self.covers_point(*corner))
    }

    /// Whether this fresh fit serves any point of its reach that `drawn` (the same cascade marched for an earlier view)
    /// no longer covers. The reach seen along the light is the convex hull of its corners in this square's plane;
    /// clipped to this square it is exactly what a fresh march would serve, and `drawn` must hold every vertex of that
    /// (convex) polygon. Points outside this square too (a narrow lens's fade bands can outrun the square fitted to the
    /// bare slice) are nobody's to serve and count for nothing.
    pub fn escapes(&self, drawn: &Cascade) -> bool {
        let plane = self.reach.map(|corner| {
            let clip = self.matrix * corner.extend(1.0);
            Vec2::new(clip.x, clip.y) / clip.w
        });
        clip_to_square(&convex_hull(&plane)).iter().any(|point| {
            // Back to the world at mid depth: the light's projection is orthographic, so the depth does not move the
            // point in `drawn`'s plane (the sun is the same for both: a sun that moves is a cut).
            let world = self.inverse_matrix.project_point3(point.extend(0.5));
            let clip = drawn.matrix * world.extend(1.0);
            // A whisker of slack: the clipped polygon's vertices sit on this square's edges, and the round trip
            // through the world rounds them a few ulps past a square that is the same.
            let reach = clip.w * (1.0 + EDGE_SLACK);
            clip.x.abs() > reach || clip.y.abs() > reach
        })
    }
}

/// How far past a marched square's edge (in its clip units, 1 = the half-width) a point may lie and still count as
/// covered by [`Cascade::escapes`]: rounding only, a few thousandths of a texel at any tier's map size.
const EDGE_SLACK: f32 = 1e-5;

/// The most vertices a [`Polygon`] holds: the hull of a reach's 8 corners has at most 8, and each of the square's 4
/// edges adds at most one in the clip (12).
const POLYGON_VERTICES: usize = 16;

/// A small polygon on the stack: [`Cascade::escapes`] runs for each cascade every update, and allocated about 13 times
/// a call when these were `Vec`s (39 of a quiet frame's allocations, `naval-client`'s `tests/frame_alloc.rs`).
#[derive(Clone, Copy, Debug)]
struct Polygon {
    points: [Vec2; POLYGON_VERTICES],
    len: usize,
}

impl Polygon {
    const EMPTY: Self = Self {
        points: [Vec2::ZERO; POLYGON_VERTICES],
        len: 0,
    };

    fn push(&mut self, point: Vec2) {
        debug_assert!(self.len < POLYGON_VERTICES, "a polygon past its room");
        if self.len < POLYGON_VERTICES {
            self.points[self.len] = point;
            self.len += 1;
        }
    }

    fn pop(&mut self) {
        self.len = self.len.saturating_sub(1);
    }
}

impl std::ops::Deref for Polygon {
    type Target = [Vec2];

    fn deref(&self) -> &[Vec2] {
        &self.points[..self.len]
    }
}

/// The convex hull of a few points (at most [`POLYGON_VERTICES`]), counter-clockwise (Andrew's monotone chain).
fn convex_hull(points: &[Vec2]) -> Polygon {
    let mut sorted = Polygon::EMPTY;
    for &point in points {
        sorted.push(point);
    }
    let len = sorted.len;
    sorted.points[..len].sort_unstable_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    // Duplicates out, in place.
    let mut kept = 0;
    for index in 0..len {
        if kept == 0 || sorted.points[index] != sorted.points[kept - 1] {
            sorted.points[kept] = sorted.points[index];
            kept += 1;
        }
    }
    sorted.len = kept;
    if sorted.len < 3 {
        return sorted;
    }
    let cross = |o: Vec2, a: Vec2, b: Vec2| (a - o).perp_dot(b - o);
    let mut hull = Polygon::EMPTY;
    for pass in 0..2 {
        let start = hull.len;
        for step in 0..sorted.len {
            let point = if pass == 0 {
                sorted.points[step]
            } else {
                sorted.points[sorted.len - 1 - step]
            };
            while hull.len >= start + 2
                && cross(hull.points[hull.len - 2], hull.points[hull.len - 1], point) <= 0.0
            {
                hull.pop();
            }
            hull.push(point);
        }
        hull.pop();
    }
    hull
}

/// A convex polygon clipped to the square [−1, 1]² (Sutherland–Hodgman).
fn clip_to_square(polygon: &[Vec2]) -> Polygon {
    // Each edge of the square: a point is inside where `inside(p) >= 0`.
    let edges: [fn(Vec2) -> f32; 4] = [|p| 1.0 - p.x, |p| p.x + 1.0, |p| 1.0 - p.y, |p| p.y + 1.0];
    let mut output = Polygon::EMPTY;
    for &point in polygon {
        output.push(point);
    }
    for inside in edges {
        let input = std::mem::replace(&mut output, Polygon::EMPTY);
        for (index, &current) in input.iter().enumerate() {
            let previous = input[(index + input.len() - 1) % input.len()];
            let (a, b) = (inside(previous), inside(current));
            if b >= 0.0 {
                if a < 0.0 {
                    output.push(previous + (current - previous) * (a / (a - b)));
                }
                output.push(current);
            } else if a >= 0.0 {
                output.push(previous + (current - previous) * (a / (a - b)));
            }
        }
        if output.is_empty() {
            break;
        }
    }
    output
}

/// The four corners of a frustum's near and far planes in view space, ordered (+x +y), (+x −y), (−x −y), (−x +y).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrustumCorners {
    pub near: [Vec3; 4],
    pub far: [Vec3; 4],
}

const CORNERS: [(f32, f32); 4] = [(1.0, 1.0), (1.0, -1.0), (-1.0, -1.0), (-1.0, 1.0)];

impl FrustumCorners {
    /// A camera's frustum (`projection` with the game's reversed depth: 1 at the near plane, 0 at the far), cut at
    /// `far` metres when that is nearer than its own far plane (`setFromCamera`).
    pub fn from_projection(projection: Mat4, far: f32) -> Self {
        let inverse = projection.inverse();
        let unproject = |x: f32, y: f32, depth: f32| {
            let point = inverse * Vec4::new(x, y, depth, 1.0);
            point.truncate() / point.w
        };
        let orthographic = projection.w_axis.w == 1.0 && projection.z_axis.w == 0.0;
        let near = CORNERS.map(|(x, y)| unproject(x, y, 1.0));
        let far = CORNERS.map(|(x, y)| {
            let mut corner = unproject(x, y, 0.0);
            let depth = corner.z.abs();
            if orthographic {
                corner.z *= (far / depth).min(1.0);
            } else {
                corner *= (far / depth).min(1.0);
            }
            corner
        });
        Self { near, far }
    }

    /// The slices between the given fractions (`split`): slice i runs from the previous fraction to fraction i.
    pub fn split(&self, fractions: &[f32]) -> Vec<FrustumCorners> {
        let lerp = |i: usize, t: f32| self.near[i].lerp(self.far[i], t);
        (0..fractions.len())
            .map(|index| FrustumCorners {
                near: std::array::from_fn(|i| {
                    if index == 0 {
                        self.near[i]
                    } else {
                        lerp(i, fractions[index - 1])
                    }
                }),
                far: std::array::from_fn(|i| {
                    if index == fractions.len() - 1 {
                        self.far[i]
                    } else {
                        lerp(i, fractions[index])
                    }
                }),
            })
            .collect()
    }
}

/// The split fractions of `count` slices between `near` and `far`, each a fraction of `far` (`practical`, the mix
/// of uniform and logarithmic splits by `lambda`; GPU Gems 3, chapter 10).
pub fn practical_splits(count: usize, near: f32, far: f32, lambda: f32) -> Vec<f32> {
    (0..count)
        .map(|i| {
            let step = (i + 1) as f32 / count as f32;
            let uniform = (near + (far - near) * step) / far;
            let logarithmic = (near * (far / near).powf(step)) / far;
            uniform + (logarithmic - uniform) * lambda
        })
        .collect()
}

/// three's `Matrix4.lookAt(eye, target, up)`: the rotation whose z axis points from `target` to `eye`, nudged
/// rather than degenerate when the view runs along `up`.
pub fn look_at_rotation(eye: Vec3, target: Vec3, up: Vec3) -> Mat4 {
    let mut z = eye - target;
    if z.length_squared() == 0.0 {
        z.z = 1.0;
    }
    z = z.normalize();
    let mut x = up.cross(z);
    if x.length_squared() == 0.0 {
        if up.z.abs() == 1.0 {
            z.x += 0.0001;
        } else {
            z.z += 0.0001;
        }
        z = z.normalize();
        x = up.cross(z);
    }
    x = x.normalize();
    let y = z.cross(x);
    Mat4::from_cols(x.extend(0.0), y.extend(0.0), z.extend(0.0), Vec4::W)
}

/// The cascades' settings (`CascadedShadowMapsOptions`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CascadeSettings {
    pub count: usize,
    /// Texels per edge of each map.
    pub map_size: u32,
    /// The farthest the cascades reach (m), or the camera's far times `far_scale`.
    pub max_far: Option<f32>,
    pub far_scale: f32,
    pub split_lambda: f32,
    pub margin: f32,
    /// Grow each square by the fade between cascades.
    pub fade: bool,
    /// How far along the view the last cascade still serves (m): the marches sample it out to their longest ray, past
    /// the camera's far plane, since the faded cascade index never fades the last cascade out. Its `reach` runs there.
    pub reach_far: f32,
}

impl Default for CascadeSettings {
    /// `CloudsEffect`'s: three cascades, the practical split at λ 0.6.
    fn default() -> Self {
        Self {
            count: 3,
            map_size: 512,
            max_far: None,
            far_scale: 1.0,
            split_lambda: 0.6,
            margin: 0.0,
            fade: true,
            reach_far: 0.0,
        }
    }
}

/// The cascades for one view (`CascadedShadowMaps.update`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cascades {
    pub cascades: [Cascade; MAX_CASCADES],
    pub count: usize,
    /// The cascades' far (m).
    pub far: f32,
}

impl Cascades {
    /// Fits the cascades to a camera (`camera_world`: view → world; `projection` without temporal AA's jitter) and
    /// `sun` (unit, toward the sun, world), the light camera `distance` metres back along it.
    pub fn fit(
        settings: &CascadeSettings,
        camera_world: Mat4,
        projection: Mat4,
        near: f32,
        camera_far: f32,
        sun: Vec3,
        distance: f32,
    ) -> Self {
        let count = settings.count.clamp(1, MAX_CASCADES);
        let far = match settings.max_far {
            Some(max_far) => max_far.min(camera_far * settings.far_scale),
            None => camera_far * settings.far_scale,
        };
        let splits = practical_splits(count, near, far, settings.split_lambda);
        let whole = FrustumCorners::from_projection(projection, far);
        let frusta = whole.split(&splits);

        let light_orientation = look_at_rotation(Vec3::ZERO, -sun, Vec3::Y);
        let camera_to_light = light_orientation.inverse() * camera_world;
        let mut cascades = [Cascade::default(); MAX_CASCADES];
        for (index, frustum) in frusta.iter().enumerate() {
            let cascade = &mut cascades[index];
            cascade.interval = [
                if index == 0 { 0.0 } else { splits[index - 1] },
                splits[index],
            ];
            let [start, end] = cascade.interval;
            let last = index + 1 == count;
            let (start, end) = (
                (start - start * start * 0.25).max(0.0),
                if last {
                    (settings.reach_far / far).max(1.0)
                } else {
                    (end + end * end * 0.25).min(1.0)
                },
            );
            for corner in 0..4 {
                let at = |t: f32| {
                    camera_world.transform_point3(whole.near[corner].lerp(whole.far[corner], t))
                };
                cascade.reach[corner] = at(start);
                cascade.reach[corner + 4] = at(end);
            }

            // The square's radius: the longer of the far plane's diagonal and the slice's, grown by the fade.
            let mut diagonal = frustum.far[0]
                .distance(frustum.far[2])
                .max(frustum.far[0].distance(frustum.near[2]));
            if settings.fade {
                let depth = frustum.far[0].z / (far - near);
                diagonal += 0.25 * depth * depth * (far - near);
            }
            let radius = diagonal * 0.5;
            // Clip depth 0 at the near plane: the shadow pass starts each texel's ray there.
            let projection_matrix = glam::camera::rh::proj::directx::orthographic(
                -radius,
                radius,
                -radius,
                radius,
                -settings.margin,
                radius * 2.0 + settings.margin,
            );

            // The slice in the light's frame: its bounds' centre, on the side toward the sun.
            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = Vec3::splat(f32::NEG_INFINITY);
            for corner in frustum.near.iter().chain(frustum.far.iter()) {
                let light = camera_to_light.transform_point3(*corner);
                min = min.min(light);
                max = max.max(light);
            }
            let mut center = (min + max) * 0.5;
            center.z = max.z + settings.margin;
            // Whole texels in the light's frame.
            let texel = radius * 2.0 / settings.map_size as f32;
            center.x = (center.x / texel).round() * texel;
            center.y = (center.y / texel).round() * texel;

            let center = light_orientation.transform_point3(center);
            let position = center + sun * distance;
            let inverse_view =
                Mat4::from_translation(position) * look_at_rotation(center, position, Vec3::Y);
            let view = inverse_view.inverse();
            cascade.matrix = projection_matrix * view;
            cascade.inverse_matrix = inverse_view * projection_matrix.inverse();
        }
        Self {
            cascades,
            count,
            far,
        }
    }
}

/// How far the light camera stands back along the sunlight (m): 1 km with the sun overhead, 1,000 km at the horizon
/// (`CloudsEffect.updateSharedUniforms`).
pub fn light_distance(sun_up: f32) -> f32 {
    1e6 + (1e3 - 1e6) * sun_up
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn practical_splits_mix_uniform_and_logarithmic() {
        let splits = practical_splits(3, 0.5, 60_000.0, 0.6);
        let expected = |step: f32| {
            let uniform = (0.5 + (60_000.0 - 0.5) * step) / 60_000.0;
            let logarithmic = 0.5 * (120_000.0f32).powf(step) / 60_000.0;
            uniform * 0.4 + logarithmic * 0.6
        };
        for (i, split) in splits.iter().enumerate() {
            assert!((split - expected((i + 1) as f32 / 3.0)).abs() < 1e-5);
        }
        assert!((splits[2] - 1.0).abs() < 1e-6);
        assert!(splits[0] < splits[1] && splits[1] < splits[2]);
    }

    fn camera() -> (Mat4, Mat4) {
        let camera = naval_render::Camera {
            eye: Vec3::new(120.0, 30.0, -40.0),
            target: Vec3::new(900.0, 400.0, -3000.0),
            fov_y_degrees: 52.0,
            near: 0.5,
            far: 60_000.0,
            curvature: naval_render::camera::EARTH_CURVATURE,
        };
        (camera.view().inverse(), camera.projection(16.0 / 9.0))
    }

    #[test]
    fn cascades_cover_their_slices_and_invert() {
        let (camera_world, projection) = camera();
        let sun = Vec3::new(0.3, 0.8, -0.5).normalize();
        let settings = CascadeSettings::default();
        let fitted = Cascades::fit(
            &settings,
            camera_world,
            projection,
            0.5,
            60_000.0,
            sun,
            light_distance(sun.y),
        );
        assert_eq!(fitted.count, 3);
        let frusta = FrustumCorners::from_projection(projection, fitted.far)
            .split(&practical_splits(3, 0.5, fitted.far, 0.6));
        for (cascade, frustum) in fitted.cascades.iter().zip(frusta.iter()) {
            // Every corner of the slice falls inside the cascade's square.
            for corner in frustum.near.iter().chain(frustum.far.iter()) {
                let world = camera_world.transform_point3(*corner);
                let clip = cascade.matrix.project_point3(world);
                assert!(
                    clip.x.abs() <= 1.0 + 1e-3 && clip.y.abs() <= 1.0 + 1e-3,
                    "{clip}"
                );
            }
            // The inverse takes clip back to the world.
            let point = Vec3::new(0.3, -0.6, 0.25);
            let round_trip = cascade
                .matrix
                .project_point3(cascade.inverse_matrix.project_point3(point));
            assert!((round_trip - point).length() < 1e-3, "{round_trip}");
            // The near plane (clip depth 0) stands back from the slice toward the sun.
            let near = cascade
                .inverse_matrix
                .project_point3(Vec3::new(0.0, 0.0, 0.0));
            let along = |p: Vec3| p.dot(sun);
            let slice_most = frustum
                .near
                .iter()
                .chain(frustum.far.iter())
                .map(|corner| along(camera_world.transform_point3(*corner)))
                .fold(f32::NEG_INFINITY, f32::max);
            assert!(along(near) >= slice_most + light_distance(sun.y) - 1.0);
        }
        assert_eq!(fitted.cascades[0].interval[0], 0.0);
        assert_eq!(fitted.cascades[2].interval[1], 1.0);
    }

    #[test]
    fn centres_snap_to_whole_texels() {
        let (camera_world, projection) = camera();
        let sun = Vec3::new(0.0, 1.0, 0.0);
        let settings = CascadeSettings::default();
        let a = Cascades::fit(
            &settings,
            camera_world,
            projection,
            0.5,
            60_000.0,
            sun,
            1000.0,
        );
        let nudged = Mat4::from_translation(Vec3::new(0.37, 0.0, 0.21)) * camera_world;
        let b = Cascades::fit(&settings, nudged, projection, 0.5, 60_000.0, sun, 1000.0);
        // A small move shifts the map by whole texels or not at all: the world origin lands on the same sub-texel.
        for (ca, cb) in a.cascades.iter().zip(b.cascades.iter()).take(3) {
            let texel = |m: &Mat4| {
                let clip = m.project_point3(Vec3::ZERO);
                (clip.truncate() * 0.5 * settings.map_size as f32).fract()
            };
            let (ta, tb) = (texel(&ca.matrix), texel(&cb.matrix));
            assert!((ta - tb).abs().max_element() < 0.02, "{ta} {tb}");
        }
    }

    /// The round robin's coverage test: a cascade covers its own view's slice and a slightly turned view's, but not
    /// the view after a turn just under the cut's 0.5 radians (Codex's case: 52°, the sun overhead).
    #[test]
    fn a_cascade_covers_small_turns_but_not_large_ones() {
        let (camera_world, projection) = camera();
        let sun = Vec3::Y;
        let settings = CascadeSettings::default();
        let fit = |camera_world: Mat4| {
            Cascades::fit(
                &settings,
                camera_world,
                projection,
                0.5,
                60_000.0,
                sun,
                1000.0,
            )
        };
        let turned = |angle: f32| {
            let eye = camera_world.w_axis.truncate();
            Mat4::from_translation(eye)
                * Mat4::from_rotation_y(angle)
                * Mat4::from_translation(-eye)
                * camera_world
        };
        let drawn = fit(camera_world);
        for (cascade, slice) in drawn
            .cascades
            .iter()
            .zip(drawn.cascades.iter())
            .take(drawn.count)
        {
            assert!(cascade.covers(&slice.reach));
        }
        let small = fit(turned(0.01));
        for index in 0..drawn.count {
            assert!(
                drawn.cascades[index].covers(&small.cascades[index].reach),
                "cascade {index}, 0.01 rad"
            );
        }
        let large = fit(turned(0.49));
        assert!(!drawn.cascades[0].covers(&large.cascades[0].reach));
        assert!(large.cascades[0].escapes(&drawn.cascades[0]));
        assert!(!small.cascades[0].escapes(&drawn.cascades[0]));
    }

    /// A still view never escapes its own cascades, even through binoculars under a high sun, where the fade bands can
    /// reach past the square fitted to the bare slice (Codex's case: about 7°, 16:9, the sun 75° up).
    #[test]
    fn a_still_narrow_view_never_escapes() {
        let camera = naval_render::Camera {
            eye: Vec3::new(0.0, 30.0, 0.0),
            target: Vec3::new(0.0, 30.0, -10_000.0),
            fov_y_degrees: 7.0,
            near: 0.5,
            far: 60_000.0,
            curvature: naval_render::camera::EARTH_CURVATURE,
        };
        let (camera_world, projection) = (camera.view().inverse(), camera.projection(16.0 / 9.0));
        let elevation = 75f32.to_radians();
        let sun = Vec3::new(0.0, elevation.sin(), -elevation.cos());
        let settings = CascadeSettings::default();
        let a = Cascades::fit(
            &settings,
            camera_world,
            projection,
            0.5,
            60_000.0,
            sun,
            1000.0,
        );
        let b = Cascades::fit(
            &settings,
            camera_world,
            projection,
            0.5,
            60_000.0,
            sun,
            1000.0,
        );
        // The case itself: some fresh cascade does not hold all of its own reach.
        assert!((0..a.count).any(|index| !a.cascades[index].covers(&a.cascades[index].reach)));
        for index in 0..a.count {
            assert!(
                !b.cascades[index].escapes(&a.cascades[index]),
                "cascade {index}"
            );
        }
    }

    /// The same narrow lens 1,300 m up moving 200 m forward (under the cut's 250 m): the fresh maps serve points the
    /// marched ones miss, along the edges next to the reach's corners that leave the square (Codex's case).
    #[test]
    fn a_moving_narrow_view_escapes() {
        let at = |z: f32| {
            let camera = naval_render::Camera {
                eye: Vec3::new(0.0, 1300.0, z),
                target: Vec3::new(0.0, 1300.0, z - 10_000.0),
                fov_y_degrees: 7.0,
                near: 0.5,
                far: 60_000.0,
                curvature: naval_render::camera::EARTH_CURVATURE,
            };
            (camera.view().inverse(), camera.projection(16.0 / 9.0))
        };
        let elevation = 75f32.to_radians();
        let sun = Vec3::new(0.0, elevation.sin(), -elevation.cos());
        let settings = CascadeSettings::default();
        let fit = |(camera_world, projection): (Mat4, Mat4)| {
            Cascades::fit(
                &settings,
                camera_world,
                projection,
                0.5,
                60_000.0,
                sun,
                1000.0,
            )
        };
        let (drawn, moved) = (fit(at(0.0)), fit(at(-200.0)));
        assert!(
            (0..drawn.count).any(|index| moved.cascades[index].escapes(&drawn.cascades[index]))
        );
    }

    /// The last cascade serves the marches out to their longest ray, past the camera's far plane: a narrow view under a
    /// low sun that turns a little leaves the last cascade's square out there (Codex's case: 7°, the sun 10° up), and a
    /// still view does not.
    #[test]
    fn the_last_cascade_reaches_past_the_far_plane() {
        let at = |angle: f32| {
            let camera = naval_render::Camera {
                eye: Vec3::new(0.0, 30.0, 0.0),
                target: Vec3::new(10_000.0 * angle.sin(), 30.0, -10_000.0 * angle.cos()),
                fov_y_degrees: 7.0,
                near: 0.5,
                far: 60_000.0,
                curvature: naval_render::camera::EARTH_CURVATURE,
            };
            (camera.view().inverse(), camera.projection(16.0 / 9.0))
        };
        let elevation = 10f32.to_radians();
        // The sun ahead of the camera, low.
        let sun = Vec3::new(0.0, elevation.sin(), -elevation.cos());
        let fit = |reach_far: f32, (camera_world, projection): (Mat4, Mat4)| {
            let settings = CascadeSettings {
                reach_far,
                ..CascadeSettings::default()
            };
            Cascades::fit(
                &settings,
                camera_world,
                projection,
                0.5,
                60_000.0,
                sun,
                1000.0,
            )
        };
        let drawn = fit(200_000.0, at(0.0));
        let last = drawn.count - 1;
        assert!(!fit(200_000.0, at(0.0)).cascades[last].escapes(&drawn.cascades[last]));
        // Turned 0.05 rad: only the reach past the far plane leaves the marched square.
        let turned = fit(200_000.0, at(0.05));
        assert!(turned.cascades[last].escapes(&drawn.cascades[last]));
        let (near_drawn, near_turned) = (fit(0.0, at(0.0)), fit(0.0, at(0.05)));
        assert!(!near_turned.cascades[last].escapes(&near_drawn.cascades[last]));
        // The reach itself runs to the longest ray.
        let eye = Vec3::new(0.0, 30.0, 0.0);
        let farthest = drawn.cascades[last]
            .reach
            .iter()
            .map(|c| c.distance(eye))
            .fold(0.0, f32::max);
        assert!(farthest > 190_000.0, "{farthest}");
    }

    #[test]
    fn hull_and_clip() {
        let square = [
            Vec2::new(-1.0, -1.0),
            Vec2::new(1.0, -1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(-1.0, 1.0),
        ];
        let mut points = square.to_vec();
        points.push(Vec2::ZERO);
        assert_eq!(convex_hull(&points).len(), 4);
        // A square straddling the right edge keeps its left half.
        let shifted: Vec<Vec2> = square.iter().map(|p| *p + Vec2::new(1.0, 0.0)).collect();
        let clipped = clip_to_square(&shifted);
        assert!(clipped.iter().all(|p| p.x >= -1e-6 && p.x <= 1.0 + 1e-6));
        assert!(clipped.iter().any(|p| (p.x - 1.0).abs() < 1e-6));
        assert!(
            clip_to_square(
                &shifted
                    .iter()
                    .map(|p| *p + Vec2::new(5.0, 0.0))
                    .collect::<Vec<_>>()
            )
            .is_empty()
        );
    }
}
