// Where the opaque scene's depth copy holds any surface, for the screen-space reflections (native only; the web traced
// every ray): `occupancy` marks each REFLECTION_TILE² pixel tile holding a surface nearer than the reach (a depth above
// `reach.x`: the sky and the far plane are 0 in the reversed depth, and the land drawn past a battle view's far plane is
// below it), and `sum` adds the marks up into a summed-area table, in a storage buffer
// (the surface's fragment stage has no sampled texture to spare). With it the surface asks in four reads whether a ray's
// screen box crosses any surface at all (`naval_ocean::screen_reflection`, `surface_between`): a ray whose box crosses
// none would read sky at every step and find nothing, so it is not traced. `reflection_tiles.rs` runs both after the
// opaque copies, every frame the surface reads them.

// Pixels per tile edge: `REFLECTION_TILE` in screen_reflection.wgsl and `reflection_tiles::TILE` (a test holds the three
// together).
const REFLECTION_TILE: u32 = 16u;

@group(0) @binding(0) var depth: texture_depth_2d;
// One mark per tile, row by row; `sum` turns each row into its running sum.
@group(0) @binding(1) var<storage, read_write> tiles: array<u32>;
// The summed-area table: its width and height (the tiles + 1 each way), then its rows; entry (x + 1, y + 1) counts the
// tiles x' ≤ x and y' ≤ y, and row and column 0 are zero.
@group(0) @binding(2) var<storage, read_write> table: array<u32>;
// x: the reversed depth of the farthest surface marked (`reflection_tiles::REACH_M`; 0 marks every surface).
@group(0) @binding(3) var<uniform> reach: vec4<f32>;

var<workgroup> marked: atomic<u32>;

fn tile_counts() -> vec2<u32> {
    return (textureDimensions(depth) + vec2<u32>(REFLECTION_TILE - 1u)) / REFLECTION_TILE;
}

// One workgroup per tile, an invocation per pixel.
@compute @workgroup_size(16, 16, 1)
fn occupancy(
    @builtin(global_invocation_id) pixel: vec3<u32>,
    @builtin(workgroup_id) tile: vec3<u32>,
    @builtin(local_invocation_index) index: u32,
) {
    if index == 0u {
        atomicStore(&marked, 0u);
    }
    workgroupBarrier();
    if all(pixel.xy < textureDimensions(depth)) && textureLoad(depth, vec2<i32>(pixel.xy), 0) > reach.x {
        atomicStore(&marked, 1u);
    }
    workgroupBarrier();
    if index == 0u {
        tiles[tile.y * tile_counts().x + tile.x] = atomicLoad(&marked);
    }
}

// One workgroup: the rows' running sums, then the columns' down them into the table.
@compute @workgroup_size(256, 1, 1)
fn sum(@builtin(local_invocation_index) index: u32) {
    let counts = tile_counts();
    for (var y = index; y < counts.y; y += 256u) {
        var run = 0u;
        for (var x = 0u; x < counts.x; x += 1u) {
            run += tiles[y * counts.x + x];
            tiles[y * counts.x + x] = run;
        }
    }
    storageBarrier();
    let width = counts.x + 1u;
    if index == 0u {
        table[0] = width;
        table[1] = counts.y + 1u;
    }
    for (var x = index; x < width; x += 256u) {
        var run = 0u;
        table[2u + x] = 0u;
        for (var y = 0u; y < counts.y; y += 1u) {
            if x > 0u {
                run += tiles[y * counts.x + x - 1u];
            }
            table[2u + (y + 1u) * width + x] = run;
        }
    }
}
