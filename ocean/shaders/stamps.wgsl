// Foam stamps painted into a coverage atlas: a port of the web game's `WakeFoamGpu.ts` (`WakeFoamGpu`'s instanced quads and
// fragment node). Each instance is a texel-aligned quad around one max-coverage ellipse (or ring), blended by `max`, so
// overlapping stamps never build an opaque stripe. Red holds the churned water (or a torpedo's bubbles), green the slick;
// blue and alpha how central each point is in its band (`TRACK_*`, read by naval_ocean::wake's `track_coordinate`).

// How far across its band (in the band's half-widths) the coordinate reaches, and its encoding: 0 where no band stamp
// covers a texel, 1 on the centreline, TRACK_ZERO at TRACK_RANGE and beyond. Max blending keeps the most central band
// where bands overlap (a ship crossing her own trail), and filtering toward an empty texel reads as the band's edge.
const TRACK_RANGE: f32 = 1.3;
const TRACK_ZERO: f32 = 0.1;

struct StampIn {
    @builtin(vertex_index) corner: u32,
    // Quad centre and half size in the atlas's NDC.
    @location(0) box_ndc: vec4<f32>,
    // Quad half-extent in the stamp's across/along axes, per NDC axis.
    @location(1) axes: vec4<f32>,
    // Quad centre in the stamp's frame (across, along), strength, ring + 2 × channel.
    @location(2) shape: vec4<f32>,
    // The stamp's centre across its band and its half-width there, in the band's half-widths (y 0: no band); z and w how
    // far the quad reaches past the stamp across and along it (its coordinates are in that grown extent).
    @location(3) band: vec4<f32>,
}

struct StampOut {
    @builtin(position) position: vec4<f32>,
    // The quad corner in [-1, 1]², y as the stamp's coordinates run (down the atlas).
    @location(0) local: vec2<f32>,
    @location(1) axes: vec4<f32>,
    @location(2) shape: vec4<f32>,
    @location(3) band: vec4<f32>,
}

@vertex
fn vertex(input: StampIn) -> StampOut {
    // Two triangles over the quad's corners.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    let c = corners[input.corner];
    var output: StampOut;
    // Atlas row zero at NDC +Y, so the water samples the tile the stamps were laid in.
    output.position = vec4<f32>(input.box_ndc.xy * vec2<f32>(1.0, -1.0) + c * input.box_ndc.zw, 0.0, 1.0);
    // Flip the stamp coordinates too.
    output.local = c * vec2<f32>(1.0, -1.0);
    output.axes = input.axes;
    output.shape = input.shape;
    output.band = input.band;
    return output;
}

@fragment
fn fragment(input: StampOut) -> @location(0) vec4<f32> {
    // In the grown quad's extent, then the stamp's own half-extents (a band's quad reaches `band.zw` times past it).
    let grown = vec2<f32>(dot(input.local, input.axes.xy) + input.shape.x, dot(input.local, input.axes.zw) + input.shape.y);
    let across = grown.x * max(input.band.z, 1.0);
    let along = grown.y * max(input.band.w, 1.0);
    let radius = across * across + along * along;
    let r = (sqrt(radius) - 0.75) / 0.13;
    let ring = exp(-(r * r));
    // shape.w holds ring + 2 × channel. Max blending leaves the other channel as it was.
    let slick = input.shape.w > 1.5;
    let rim = input.shape.w - select(0.0, 2.0, slick) > 0.5;
    let profile = select(1.0 - smoothstep(0.0, 1.0, radius), ring, rim);
    let coverage = select(0.0, profile * input.shape.z, radius < 1.0);
    // How central the point is in the band: the same for every stamp of a band at a point; written over the grown
    // quad's ellipse.
    let off_centre = abs(input.band.x + across * input.band.y);
    let reached = dot(grown, grown) < 1.0 && input.band.y > 0.0;
    let track = select(0.0, TRACK_ZERO + (1.0 - TRACK_ZERO) * (1.0 - min(off_centre / TRACK_RANGE, 1.0)), reached);
    return vec4<f32>(select(coverage, 0.0, slick), select(0.0, coverage, slick), select(track, 0.0, slick), select(0.0, track, slick));
}
