use super::*;

const NONE: f32 = f32::NAN;

/// A grid of `columns` × `rows` 70 m samples from (x, z) = (1000, -500), every sample `f(c, r)`.
fn grid(columns: usize, rows: usize, f: impl Fn(usize, usize) -> f32) -> LakeGrid {
    let mut levels = Vec::with_capacity(columns * rows);
    for r in 0..rows {
        for c in 0..columns {
            levels.push(f(c, r));
        }
    }
    LakeGrid {
        origin: [1000.0, -500.0],
        cell: 70.0,
        columns,
        rows,
        levels,
    }
}

/// The squares (between samples, indexed from -1 as the mesh does) each quad of `mesh` covers, with its level: an error
/// if two quads cover one square.
fn covered(mesh: &LakeMesh, g: &LakeGrid) -> std::collections::HashMap<(i64, i64), f32> {
    let mut out = std::collections::HashMap::new();
    for q in mesh.indices.chunks(6) {
        let a = mesh.vertices[q[0] as usize].position;
        let c = mesh.vertices[q[2] as usize].position;
        let to = |x: f32, o: f64| ((f64::from(x) - o) / g.cell).round() as i64;
        let (x0, x1) = (to(a[0], g.origin[0]), to(c[0], g.origin[0]));
        let (z0, z1) = (to(a[2], g.origin[1]), to(c[2], g.origin[1]));
        assert!(x1 > x0 && z1 > z0, "a quad's corners in order: {a:?} {c:?}");
        assert!(x1 - x0 <= QUAD_CELLS as i64 && z1 - z0 <= QUAD_CELLS as i64);
        for z in z0..z1 {
            for x in x0..x1 {
                assert!(
                    out.insert((x, z), a[1]).is_none(),
                    "square ({x}, {z}) twice"
                );
            }
        }
    }
    out
}

#[test]
fn one_sample_is_water_over_the_squares_about_it() {
    let g = grid(3, 3, |c, r| if (c, r) == (1, 1) { 12.5 } else { NONE });
    let mesh = LakeMesh::build(&g);
    assert_eq!(mesh.lakes, 1);
    // The four squares about the sample, merged into one quad from its neighbours' samples to theirs.
    assert_eq!(mesh.indices.len(), 6);
    let squares = covered(&mesh, &g);
    assert_eq!(squares.len(), 4);
    for (x, z) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        assert_eq!(squares.get(&(x, z)), Some(&12.5));
    }
    let chunk = &mesh.chunks[0];
    assert_eq!(chunk.min, [1000.0, 12.5, -500.0]);
    assert_eq!(chunk.max, [1140.0, 12.5, -360.0]);
    // A pond this small is fresh water with a short fetch.
    let v = mesh.vertices[0];
    assert_eq!(v.lake[0], 70.0);
    assert_eq!(v.lake[1], 0.0);
    // The mask: the sample at texel (2, 2), nothing about it.
    assert_eq!((mesh.mask.width, mesh.mask.height), (5, 5));
    assert_eq!(mesh.mask.texels.iter().filter(|&&t| t == 255).count(), 1);
    assert_eq!(mesh.mask.texels[2 * 5 + 2], 255);
    // About it the blur: its four neighbours 2/16, its diagonals 1/16, nothing two samples off.
    assert_eq!(mesh.mask.texels[2 * 5 + 1], 31);
    assert_eq!(mesh.mask.texels[5 + 1], 15);
    assert_eq!(mesh.mask.texels[0], 0);
    assert_eq!(mesh.mask.origin, [930.0, -570.0]);
}

#[test]
fn two_lakes_at_their_own_levels_leave_the_land_between_dry() {
    // Two lakes six samples apart, one a salt pond by the sea.
    let g = grid(10, 3, |c, r| match (c, r) {
        (1, 1) => 1.5,
        (8, 1) => 40.0,
        _ => NONE,
    });
    let mesh = LakeMesh::build(&g);
    assert_eq!(mesh.lakes, 2);
    let squares = covered(&mesh, &g);
    assert_eq!(squares.len(), 8);
    // No water over the squares between them (columns 2 to 6).
    assert!(squares.keys().all(|(x, _)| *x <= 1 || *x >= 7));
    assert!(squares.iter().all(|(&(x, _), &l)| (x <= 1) == (l == 1.5)));
    let salt: Vec<f32> = mesh.vertices.iter().map(|v| v.lake[1]).collect();
    assert!(
        mesh.vertices
            .iter()
            .zip(&salt)
            .all(|(v, &s)| (v.position[1] <= SALT_LEVEL_M) == (s == 1.0))
    );
}

#[test]
fn a_big_lake_is_merged_into_quads_that_cover_it_once() {
    // A 40 × 40 lake with a bay cut out of it, across chunks.
    let g = grid(42, 42, |c, r| {
        let inside = (1..41).contains(&c) && (1..41).contains(&r);
        let bay = c > 20 && r < 10;
        if inside && !bay { 80.0 } else { NONE }
    });
    let mesh = LakeMesh::build(&g);
    assert_eq!(mesh.lakes, 1);
    let squares = covered(&mesh, &g);
    // Every square with a lake corner, and no other.
    let lake = |c: i64, r: i64| {
        c >= 0 && r >= 0 && (c as usize) < g.columns && (r as usize) < g.rows && {
            g.levels[r as usize * g.columns + c as usize].is_finite()
        }
    };
    let mut expected = 0;
    for z in -1..g.rows as i64 {
        for x in -1..g.columns as i64 {
            let any = lake(x, z) || lake(x + 1, z) || lake(x, z + 1) || lake(x + 1, z + 1);
            assert_eq!(squares.contains_key(&(x, z)), any, "square ({x}, {z})");
            expected += usize::from(any);
        }
    }
    assert_eq!(squares.len(), expected);
    assert!((mesh.area() - expected as f64 * 4900.0).abs() < 1.0);
    // Far fewer quads than squares.
    assert!(
        mesh.indices.len() / 6 < expected / 20,
        "{} quads",
        mesh.indices.len() / 6
    );
    // The chunks hold disjoint runs of the indices, each within its 32-cell square of chart.
    let mut end = 0;
    for chunk in &mesh.chunks {
        assert_eq!(chunk.indices.start, end);
        end = chunk.indices.end;
        let span = (CHUNK_CELLS as f64 * g.cell) as f32;
        assert!(chunk.max[0] - chunk.min[0] <= span + 0.01);
        assert!(chunk.max[2] - chunk.min[2] <= span + 0.01);
    }
    assert_eq!(end as usize, mesh.indices.len());
    assert!(mesh.chunks.len() >= 4);
}

#[test]
fn where_two_lakes_share_a_square_the_higher_takes_it() {
    // Two lakes side by side at different levels (salt ponds behind a levee too thin for a sample).
    let g = grid(4, 1, |c, _| if c < 2 { 1.5 } else { 2.0 });
    let mesh = LakeMesh::build(&g);
    assert_eq!(mesh.lakes, 2);
    let squares = covered(&mesh, &g);
    assert_eq!(squares.len(), 5 * 2);
    // The squares between samples 1 and 2 are the higher pond's.
    assert_eq!(squares[&(1, -1)], 2.0);
    assert_eq!(squares[&(1, 0)], 2.0);
    assert_eq!(squares[&(0, 0)], 1.5);
}

#[test]
fn an_empty_grid_has_no_water() {
    let g = grid(5, 5, |_, _| NONE);
    let mesh = LakeMesh::build(&g);
    assert_eq!(mesh.lakes, 0);
    assert!(mesh.indices.is_empty() && mesh.chunks.is_empty());
}

/// A camera at `eye` looking at `target` (52°, 16:9), the planes and its position.
fn view(eye: Vec3, target: Vec3) -> (Vec3, Vec<Vec4>) {
    let camera = naval_render::Camera {
        eye,
        target,
        fov_y_degrees: 52.0,
        near: 1.0,
        far: 200_000.0,
        curvature: naval_render::camera::EARTH_CURVATURE,
    };
    let vp = camera.projection(16.0 / 9.0) * camera.view();
    (eye, frustum_planes(vp))
}

#[test]
fn chunks_are_culled_outside_the_view_and_beyond_the_haze_where_the_land_places_them() {
    // Two ponds 3 km apart along x, at (0, z = 0) and (3000, 0) once placed.
    let g = LakeGrid {
        origin: [0.0, 0.0],
        cell: 70.0,
        columns: 50,
        rows: 3,
        levels: (0..150)
            .map(|k| {
                if k % 50 == 1 || k % 50 == 44 {
                    10.0
                } else {
                    NONE
                }
            })
            .collect(),
    };
    let mesh = LakeMesh::build(&g);
    assert_eq!(mesh.chunks.len(), 2);
    let curvature = naval_render::camera::EARTH_CURVATURE;
    // From a plane 300 m up 2 km south of the first, looking north at it: the second, 3 km east, out of view.
    let offset = [0.0, 0.0];
    let (eye, planes) = view(Vec3::new(70.0, 300.0, 2000.0), Vec3::new(70.0, 0.0, 0.0));
    assert_eq!(
        visible_chunks(&mesh.chunks, offset, eye, &planes, curvature, 60_000.0),
        vec![0]
    );
    // Placed 3 km west, the land's offset: now the second stands where the first stood.
    let (eye2, planes2) = view(Vec3::new(70.0, 300.0, 2000.0), Vec3::new(70.0, 0.0, 0.0));
    let moved = visible_chunks(
        &mesh.chunks,
        [-3010.0, 0.0],
        eye2,
        &planes2,
        curvature,
        60_000.0,
    );
    assert_eq!(moved, vec![1]);
    // Beyond the haze's reach, none.
    assert!(visible_chunks(&mesh.chunks, offset, eye, &planes, curvature, 1500.0).is_empty());
    // Looking away, none.
    let (eye, planes) = view(Vec3::new(70.0, 300.0, 2000.0), Vec3::new(70.0, 0.0, 4000.0));
    assert!(visible_chunks(&mesh.chunks, offset, eye, &planes, curvature, 60_000.0).is_empty());
}

#[test]
fn a_far_lake_dropped_by_the_earths_curve_is_still_drawn() {
    // A pond 60 km off near sea level, seen from 3 km up with the view's top edge 3° below the horizon: on the plane it
    // lies 2.86° down, above the view; the camera draws it 280 m lower (curvature × 60 km²), 3.13° down, inside it.
    let g = grid(3, 3, |c, r| if (c, r) == (1, 1) { 2.0 } else { NONE });
    let mesh = LakeMesh::build(&g);
    let eye = Vec3::new(1070.0, 3000.0, 0.0);
    let pitch = (26.0f32 + 3.0).to_radians();
    let (eye, planes) = view(
        eye,
        eye + Vec3::new(0.0, -pitch.sin(), -pitch.cos()) * 1000.0,
    );
    let offset = [0.0, -59_570.0];
    let curvature = naval_render::camera::EARTH_CURVATURE;
    let flat = visible_chunks(&mesh.chunks, offset, eye, &planes, 0.0, 100_000.0);
    let curved = visible_chunks(&mesh.chunks, offset, eye, &planes, curvature, 100_000.0);
    assert!(flat.is_empty(), "the flat box is above the view");
    assert_eq!(curved, vec![0]);
}
