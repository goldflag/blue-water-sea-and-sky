# Ocean: overview for re-implementers

This is a map of `ocean/` (about 29k lines of Rust and WGSL) for someone writing their own ocean in another engine.
It states what the code does, with the formulas and constants, and points at the file and function to read for
detail. Line numbers are as of this snapshot.

## Path names in the sources

The copied sources and the dev logs use the game's monorepo paths:

| In the sources | Here |
| --- | --- |
| `native/crates/naval-ocean` | `ocean/` |
| `native/crates/naval-sky` | `sky/` |
| `native/crates/naval-clouds` | `clouds/` |
| `native/crates/naval-render` | `shared/render/` (partial) |
| `native/docs/*.md` | `docs/` |

Anything else (`src/game/*`, the "web game", `naval-client`, `naval-sim`, `naval-water-effects`, `bun native/tools/...`)
is not in this repository. The browser game the native code was ported from no longer exists; where a comment says
"ports `ocean/waves/field.ts`", the Rust/WGSL is now the only implementation. "Parity with the web" sections in the
docs are history.

Docs: `docs/ocean.md` (the ocean's dated development log, newest first), `docs/sea-sky.md` (the sea around hulls,
the hulls' wet band, the underwater view, pause), `docs/water-effects.md` (wakes, bow waves, splashes).

## 1. What it produces, and the frame's pass order

One layer, `Ocean` (`ocean/src/ocean.rs`), implements the renderer's `SceneLayer` trait
(`shared/render/src/frame.rs:345`). The renderer draws four phases (`frame.rs:98`): **Opaque** (hulls, land) →
copy of HDR colour and depth → **Water** (the sea, which samples those copies) → **Sky** (dome and clouds where
nothing was drawn; reversed depth, `GreaterEqual` against 0) → **Transparent** (spray, smoke) → backdrop copy.
HDR target `Rgba16Float`, depth `Depth32Float` reversed (cleared to 0).

Per frame, `Ocean::prepare` (`ocean.rs:650`) records, in order:

| # | Work | Where | Output |
| --- | --- | --- | --- |
| 1 | Wave field: evolve spectrum + inverse FFT rows, columns; resolve (last column pass, foam persistence); mip chains | `field.rs:1224` `update_with`; `fft.wgsl`, `resolve.wgsl`, `mips.wgsl` | three `Rgba16Float` 2D arrays (one layer per cascade), full mips to 16², ping-ponged in two sets |
| 2 | Wakes' GPU half: wake-field steps (30 Hz, ≤ 4 a frame), trail/torpedo stamp atlases, uniform upload | `wake/mod.rs`, `trail.rs`, `stamps.rs` | wake display textures, trail atlas |
| 3 | Sea height on a patch about each coupled hull (for the hulls' wet band) | `hull_heights.rs`, `hull_heights.wgsl` | `R32Float` array, one layer per hull |
| 4 | While raining: sea height on a 256 m square about the camera (for the clouds crate's rain splashes) | `rain_heights.rs` | 1 m/texel height map |
| 5 | Camera below 25 m: caustics layers | `caustics.rs`, `caustics.wgsl` | 4 levels, 256², `Rgba16Float` |
| 6 | Near plane may be under water: underwater probe | `underwater.rs`, `underwater_probe.wgsl` | 128 × 72 `Rgba16Float` (depth below waves per pixel; camera depth) |
| 7 | Surface uniforms: clipmap centre, finest level, or the "fan" mesh for magnified views | `geometry.rs`, `surface.rs:793` | — |

Then, inside the scene pass:

- `after_copies(CopyPoint::Opaque)` (`ocean.rs:817`): a compute pass marks which 16 × 16-pixel tiles of the depth
  copy hold any surface and builds a summed-area table in a storage buffer (`reflection_tiles.rs`,
  `reflection_tiles.wgsl`). Screen-space reflection rays whose screen box crosses no surface are not traced.
- `Phase::Water`: the surface, one draw pass over the clipmap (finest level's middle, then rings + horizon ring +
  skirt) or the fan (`surface.wgsl`).
- `Phase::Transparent`: spindrift sprites in gales (`spindrift.wgsl`).
- The underwater composite is its own layer (`underwater.rs` `UnderwaterLayer`): two blended full-screen draws.
- Motion pass for temporal AA: the sea writes a "response" so it keeps only 70 % history (`surface.wgsl:706`;
  without it glints shimmer, with full history they smear). Rain lowers the history further.

A paused frame (`dt == 0`, same phase, no new spectrum) skips the wave compute entirely and keeps the previous fields,
foam included (`field.rs:1270`), so a paused picture holds byte for byte.

## 2. Wave generation

### Cascades

`QualityTier::cascades` (`lib.rs`): tiles of **1024, 181, 31 m**, largest first. Low 1 × 256², Medium 2 × 256², High
3 × 256², Ultra 3 × 512². With the "realistic sea state" (the game's default) all tiles grow by one factor until the
largest holds 8 peak wavelengths, at most ×4 (`sea_state.rs` `sea_state_cascades`).

Bands are disjoint so no wavevector is drawn twice (`spectrum.rs:105` `cascade_bands`): the finer cascade takes over
at `k_cut = min(8 · 2π / L_fine, k_nyquist_coarse / 2)`, `k_nyquist = π N / L`.

### Spectrum (CPU, once per sea change)

`build_spectrum` (`spectrum.rs:261`), f64, amplitudes stored f32:

- **Energy per ω**: JONSWAP (`spectrum.rs:130`) `g²/ω⁵ · exp(−1.25 (ωp/ω)⁴) · γ^r`, σ = 0.07 / 0.09; or, for the
  realistic sea, Donelan–Hamilton–Hui's ω⁻⁴ equilibrium form (`spectrum.rs:140`) with σ = 0.08 (1 + 4 / age³).
  Above the peak both are held under Phillips' saturation `0.02 g² ω⁻⁵`, blended in (smoothstep) between 1.5 and 3 ×
  ωp (`sea_spectrum`, `spectrum.rs:151`). The level α is solved by bisection so the total variance is (Hs/4)².
- **Directional spreading**: `cos^{2s}(θ/2)` (Hasselmann 1980, `spectrum.rs:189`): s = 6.97 (ω/ωp)^4.06 below 1.05 ωp,
  else 9.77 (ω/ωp)^(−2.33 − 1.45 (age − 1.17)), age = U ωp / g clamped 0.3–3, times `directional_sharpness` (0.8).
  Normalised with Γ functions (`spreading_normalization`).
- **Per lattice pair**: density = S(ω) · g/(2ω) · D(θ) / k · Δk². One draw per ± pair from a counter-based PCG hash of
  (seed, cascade, nx, nz) (`mode_hash`), Rayleigh radius √(−2 ln u) and uniform phase; the wave is placed travelling
  with or against the wind in proportion to D(θ) vs D(θ+π). Its conjugate twin is written so the IFFT is real.
- **Time folding**: every ω is quantised to a whole multiple of 2π/4096 s (`FOLD_PERIOD`, `frequency_multiple`), so the
  GPU takes `time mod 4096` and f32 phases never drift. Stored per texel: (Re a, Im a, ±m, 0).
- After drawing, amplitudes are rescaled so the realised Hs is exact. Statistics kept for the shader: per-cascade
  variance, slope variance, "slope length" (2 × slope-weighted mean 1/|k|), and the **tail**: Cox–Munk's total slope
  variance `0.003 + 0.00512 U` minus what the cascades draw; 25 % of it roughens every pixel, the whole of it is used
  as unresolved roughness for glitter.

Why the CPU and not a compute shader: the whitecap thresholds, height bounds and tail need the realised spectrum's
statistics on the CPU (`docs/ocean.md`, "What is ported"). It costs a few ms per sea change.

### Sea parameters

- `calibration.rs` `wind_sea`: a 15-row table wind → (Hs, λp), e.g. 9 m/s → 1.8 m; choppiness
  `0.65 + 0.9 clamp((U − 3)/12)`; γ 2.6; seed 1941; wind direction in radians from +X toward +Z (waves travel toward it).
- Realistic sea (`sea_state.rs` `drawn_sea`): keeps the table's Hs but takes λp from JONSWAP's fetch-limited growth
  (`g²m0/U⁴ = 1.6e-7 χ`, `fp U/g = 3.5 χ^−0.33`): 70 m at 9 m/s, 146 m at 15, 298 m at 25. γ = 1.7 + 6 log10(age),
  choppiness = 1/(π · 0.1412) ≈ 2.25 (crests cusp at Stokes' limit), Donelan's spectrum.

### GPU FFT and the three fields

`fft.wgsl`: Stockham passes with radices ≤ 16 (`field.rs` `fft_radices`: 256 → 16·16, 512 → 8·8·8). The frequency
atlas holds all cascades side by side (N·count × N) in storage buffers, two packed complex signals per texel:
`(Dx + i·Dy, Dz + i·∂Dx/∂z)` and `(∂y/∂x + i·∂y/∂z, ∂Dx/∂x + i·∂Dz/∂z)`. The first row pass evolves the spectrum
(`a · e^{∓iωt}`) as its input; rows then columns; the last column pass is `resolve.wgsl`, which writes:

| Texture (`Rgba16Float` array, layer = cascade) | x | y | z | w |
| --- | --- | --- | --- | --- |
| `displacement` | Dx | Dy (height) | Dz | gale streak foam (cascade 1 only) |
| `derivatives` | ∂y/∂x | ∂y/∂z | ∂Dx/∂x | ∂Dz/∂z |
| `extras` | ∂Dx/∂z | foam | (∂y/∂x)² + (∂y/∂z)² | bubbles |

`mips.wgsl` box-filters each to 16². Because the squared slope is mipped with the slope, `E[s²] − E[s]²` is the slope
variance under any pixel's footprint (LEAN mapping); the shader adds it to the unresolved roughness. Sampler:
trilinear, anisotropy 16. Test: `tests/field.rs` checks the GPU displacement against a CPU inverse DFT (≤ 2e-3) and the
mip averages.

Cost: the whole field update is about 0.31 ms on an Apple M5 Pro at High (`docs/ocean.md`, "GPU cost").

### Reading the field

`waves.wgsl` (module `naval_ocean::waves`, the wave field's bind group 2):

- **Displacement** (`displacement`, `waves.wgsl:487`): each cascade read at mip `log2(spacing · texels_per_m)` where
  `spacing` is the vertex spacing, faded out between spacing = longest/4 and longest/2 (a mesh can't carry a wave
  shorter than ~4 vertices).
- **Stochastic tiling** (`scatter`, `waves.wgsl:107`; Heitz & Neyret 2018) on the two coarse cascades: a rotated
  triangular grid (cell 0.93 tiles) picks three randomly shifted copies; weights are the cubed barycentrics normalised
  so their squares sum to 1 (keeps variance of signed fields); positive fields (foam, bubbles, per-copy variance) use
  the squared weights (keeps their mean). Reads use explicit gradients from the pixel's own footprint so a random
  shift at a triangle edge never selects a coarse mip (seams). The finest cascade is not scattered.
- **Height at a world point** (`height_at`, `waves.wgsl:565`): the choppy map is not invertible in closed form; six
  fixed-point steps find the grid point whose displaced position is `xz`, damped 0.7 after the first.
  `drawn_height_at` (`:547`) does the same but at a mesh spacing, giving the height *as drawn*, which is what objects
  touching the water must use (see pitfalls).
- **Physics readback**: there is none to the CPU. The simulation never reads these waves; ships ride the simulation's
  own two-sine sea (below). GPU consumers that need heights run a compute pass that binds the field and the wakes
  (`sea_read.rs` sets up groups 0–1 for them): hull patches (`hull_heights.rs`, up to 256 × 64 texels per hull,
  about 1 m along a battleship), the underwater probe, the rain heights, caustics.

### Swell

The drawn sea's longest waves are not the FFT's. The game's simulation poses every hull on two long-crested sines
(0.7 of the amplitude at 4 × the calibrated peak wavelength along the wind, 0.3 at 0.57 of it turned 0.8 rad). The
ocean draws that sea everywhere (`hull_sea.rs`, `HullSeaUniforms::set_everywhere`) so every hull sits in the water she
rides: the first cascade is split at a mip level (`split_level`), its long part replaced by the sines, the short
waves, foam and wakes riding on top. Away from hulls each sine becomes a spread of components (`open_sea_swell`,
`hull_sea.rs:182`): 12 more for the first sine, 4 for the second, directions within ±0.8 rad, wavelengths 0.70–1.42 ×
on a golden-ratio sequence, equal energy shares, each on its own dispersion. Near a hull the spread hands its energy
back (`swell_freedom`; amplitude √(1 − f²(1 − share))) so variance holds everywhere and the hull rides exactly the
simulation's sea.

A re-implementer without a ship simulation can drop the coupling and either draw the FFT's long waves or add a
short-crested swell spread like `open_sea_swell`.

## 3. Surface geometry

`geometry.rs` (the clipmap and horizon ring, `build` at `:509`) and the vertex stage `surface.wgsl:176`.

- **Clipmap**: nested squares, each twice the extent and cell of the one inside; the outermost reaches 4096 m
  (`CLIPMAP_REACH`). Tiers (`QualityTier::clipmap`): Low 32 cells per level edge × 6 levels (finest 8 m), Medium 64 × 7
  (2 m), High 128 × 9 (0.25 m), Ultra 256 × 9 (0.125 m). Each level snaps to twice its own cell so vertices sit on
  fixed world lattice points (no swimming). Odd vertices geomorph onto the coarser lattice over 0.6–0.85 of the way to
  a level's edge, so neighbouring levels share edge vertices.
- **Finest level drawn follows the view's standoff** (`Clipmap::detail`, `geometry.rs:134`; `standoff` `:174`): the
  eye's height above the water at the centre, minus Hs, divided by the lens magnification. The first level as wide as
  that is the finest drawn; it morphs onto the next over the second half of the way so dropping it never pops. A
  periscope draws all levels (25 cm cells); a view from 1 km starts at 16 m cells. Triangles on High: about 230k from
  a periscope, 107k from 420 m up.
- **Horizon ring and skirt**: beyond 4096 m, circles 3.5 km apart to 60 km, then 15 km apart to 480 km, out to 95 %
  of the far plane; then six skirt rows from the ring to the geometric sea horizon `√(h / curvature)` when the eye is
  high enough to see past the ring. Rows past the horizon (plus a 20 m crest) are collapsed, so no hidden water is
  shaded. Earth curvature is applied per vertex (`shared/render/shaders/camera.wgsl` `curved`: y −= c · |Δxz|²); chord
  sag between 3.5 km circles is ≤ 0.25 m. Water beyond the far plane is pulled just inside it along its line of sight.
- **Magnified views (binoculars)**: from ≥ 1.25× and ≥ 10 m above the surface a polar "fan" replaces the clipmap
  (`geometry.rs:360` `fan`): columns a fixed step of bearing apart (512 columns), rows square near the eye and a fixed
  number of pixels tall beyond a knee, quantised so turning or tilting moves no vertex. Below that magnification the
  clipmap centre moves to what the lens is aimed at (`focus`, `:187`).
- **Waves on the mesh**: each vertex calls `displacement(grid, spacing)` plus the wake height. Shading uses the grid
  point (`input.grid`) for all field reads; normals come from the field per pixel, not from the mesh.

## 4. Shading (`surface.wgsl`, fragment at `:486`)

Bind groups: 0 camera + sun shadow maps + aerial volume + motion (renderer's), 1 the surface's own (uniform, foam
texture, opaque colour/depth copies at 3–4, wakes at 5–10, cloud shadow at 11–13, underwater probe 14, reflection tile
table 15), 2 wave field (`waves.wgsl`), 3 the sky's environment. Shader defs: `CASCADES`, `REFLECTION_STEPS` (0/0/16/32
per tier), `POINT_LIGHTS` (a second pipeline used only on frames with lights).

The fragment, in order:

1. **Per-pixel wave sample** (`wave_surface`, `:312`). Footprint = max(min(|ddx|, |ddy|), max/16). For each cascade:
   fade it out over the last mip (`detail`), read derivatives and extras through the scatter copies, sum slopes and
   strain, add filtered slope variance (LEAN) to the unresolved variance; once the pixel spans most of a cascade's
   slope length the expectation replaces the filtered value. World slope through the inverse transpose of the
   horizontal Jacobian: `J = (1 + ∂Dx/∂x)(1 + ∂Dz/∂z) − (∂Dx/∂z)²`,
   `slope_world = ((1+∂Dz/∂z) sx − c sz, (1+∂Dx/∂x) sz − c sx) / max(J, 0.1)`. Sub-cascade **ripples**: the finest
   cascade's derivatives re-read on a much smaller tile. **Gusts** (`gust_gain` `:266`): lognormal mean-1 gain from
   three octaves of world value noise drifting downwind, on the slope variance of waves shorter than 8 m.
2. **Normal**: slope + wake slope (+ rain rings, `rain.wgsl`). Also `footprint_variance` (`reflection.wgsl`; Kaplanyan
   2016): 0.25 × |screen derivative of slope|², capped at 0.1.
3. **Light**: the active celestial light (sun, or moon once it outshines it) from the sky's environment uniform
   (`light_direction`, `light_color × light_intensity`). Shadow = ship shadow maps (`shared/render/shaders/shadow.wgsl:455`
   `water_sun_shadow`) × clouds' shadow (`clouds/shaders/shadow.wgsl` `cloud_shadow`).
4. **Roughness**: total unresolved slope variance split along/across the wind in Cox–Munk's ratio (along `3.16e-3 U`,
   across `0.003 + 1.92e-3 U`; `wind_variances`).
5. **Reflection lobe** (`reflection_lobe`, `reflection.wgsl:138`; after Bruneton, Neyret & Holzschuch 2010): mean
   visible facet from the Gaussian slope distribution (inverse Mills ratio), shifted for Fresnel's falloff; the
   mirrored direction and its angular spread in-plane and across.
6. **Water body** (`water_body.wgsl`; Lee, Carder & Arnone 2002 remote-sensing reflectance):
   - `u = b / (a + b)`, `r = u (0.089 + 0.125 u)`, `r_rs = r / (1 − 1.7 r)`; backscatter b = (0.00323, 0.00395,
     0.00565) /m at ~620/550/460 nm; absorption a from the map or `optics.rs` profiles (clear, oceanic, productive,
     sediment, organic), optionally varying over the world (`water_properties.wgsl`).
   - `upwelling = r_rs · (E_sun · cos θs · (1 − F(θs)) · lit + E_sky · (1 − 0.066))`.
   - The opaque scene behind the water (depth copy → column length along the ray) shows through
     `T = exp(−(a + b) · d · (1 + |v_y| / 0.85))`: `body = (scene · T · 0.97 + deep · (1 − T)) / 1.333²`. This
     gives the shallows, submerged hulls and shoreline thinning with no separate refraction pass (no offset of the
     scene by the normal).
7. **Fresnel**: Schlick F0 = 0.02, averaged over the facets: `mean_fresnel` (`:92`) =
   `F0 + (1 − F0) (1 − cos)^(5 e^(−2.69σ)) / (1 + 22.7 σ^1.5)`.
8. **Sky reflection** (`sky_lobe`, `:176`): the environment probe at a PMREM roughness converted from the lobe's
   angular spread with a measured table (`pmrem_roughness`, `:99`); one tap for a round lobe, three taps (¼, ½, ¼)
   along the plane of incidence for an elongated one. Rays below the horizon read the horizon. Hull shelter
   (`wake.wgsl` `hull_shelter`) darkens sky and light beside hull sides.
9. **Screen-space reflection** (`screen_reflection.wgsl:180`; McGuire & Mara 2014): one ray traced one in-plane
   deviation below the lobe centre, 16 steps (High) or 32 (Ultra), bisection refinement, over the opaque copy only;
   result mixed over the sky by its confidence. Reads are rejected where the struck point lies below the sea drawn
   there or where the smeared image would land on a deck (see pitfalls). Skipped via the tile table when the ray's
   screen box has no surface. No planar reflection is used.
10. **Sun glitter** (`sun_glitter`, `:209`): Cox–Munk anisotropic Gaussian slope density at the half vector (plus the
    sun disc's own variance 9.38e-6 per axis for a 1.4° disc), Smith masking, Schlick at the half angle:
    `E · F · p(slope) · G / (4 cos_v cos_h⁴)`. Then a soft knee at 0.25 × sun intensity, then **glints**
    (`glitter_glints`, `:234`; `glint_cell` `:257`): expected glinting facets per pixel = density × 1.17e-4 (slope
    area mirroring part of the disc) × 400 facets/m² × pixel area on the water; draw a Poisson count (inverse CDF up to
    10, Gaussian above) per world-anchored cell about a pixel across (two power-of-two sizes blended), re-drawn 4 times a
    second with a 60 % hold; multiply the mean by count/expected. Fades in between pixel sizes 0.1 and 0.4 m. Each
    glint's pixel is capped at 1.0 × sun intensity. Plus a small `crest_transmission` term looking into the sun.
11. **Foam** (`foam.wgsl`): see below. Composited in order windrows/gale streaks → whitecaps (and wake churn) → surf →
    shoreline with `foam_over` (`:391`): foam keeps half the water's mirror (`FOAM_GLOSS` 0.5) and is never darker than
    the water it covers. Bubble clouds under foam brighten the body towards a turquoise (`bubble_cloud`, path 2.6 m,
    albedo 0.42).
12. **Point lights** (variant only; `point_lights.wgsl`): muzzle flashes, fires, lightning as glints (same BRDF,
    under the same knee), light into the water, light on foam; each light's visibility marched over the depth copy.
13. **Spray haze** in gales (`spray_haze.wgsl`), then **aerial perspective** (`naval::aerial` from
    `shared/render/shaders/aerial.wgsl`, the volume the sky fills), the same air every material takes.
14. **Underside** for back faces seen from below (`underside`, `surface.wgsl:469`): Snell's window (refract with
    n = 1.333, radiance × n²), total internal reflection of the deep outside it; foam glows milky from below.

### Foam and whitecaps

- **Where crests break** (`resolve.wgsl:130`, `whitecaps.rs`): indicator = a · compression + b · forward-face slope,
  compression `−(∂Dx/∂x wx² + 2 ∂Dx/∂z wx wz + ∂Dz/∂z wz²)`, face `−(∇h · wind)`. Both are Gaussian with variances the
  spectrum gives exactly; `cascade_breaking` (`whitecaps.rs:169`) normalises them and mixes with a 0.9 rad phase
  shift (cos, sin weights), so the indicator is a standard normal. Each cascade's threshold is the normal quantile for
  its share of the coverage Monahan & O'Muircheartaigh give: `3.84e-6 · U^3.41`, ramping on between 3.5 and 5 m/s.
  Only waves from 1/32 to 0.3 of the sea's mean wavelength break (fading by 0.6): dominant waves rarely break.
- **Persistence**: `foam = max(diffused_prev · exp(−dt/τ), injection)`, injection a smoothstep around the
  threshold; τ = 1 period of the breaking waves; bubbles last half as long. Explicit diffusion each step, coefficient
  `0.015 λ c` ≤ 1 m²/s along the wind and a quarter of that across (held below 0.24 per step for stability). Foam is
  stored in the field's texture, so it repeats with the tile; three things hide that: per-pixel `breaking_group`
  (`waves.wgsl:192`, a world-space value noise of wave groups drifting at group velocity decides which breakers show),
  gating finer cascades' foam to the crests of the coarser ones, and the stochastic tiling.
- **Compression**: shaded foam × 1/J (clamped 0.35–2.5): foam gathers where the surface is compressed.
- **Look** (`whitecap_foam`, `foam.wgsl:263`): one 256² RGBA8 texture generated at startup (`foam_texture.rs`): R lace,
  G patches (fbm), B windrow band mask, A churn (billows). R/G/A are histogram-equalised per mip, so thresholding at
  `1 − coverage` keeps the coverage at any distance. Texture tile 40 m across the wind, stretched along it. A dense
  core (fresh foam 0.45–1.4) with torn edges, lace around it, world-space value-noise tear (9, 3.7, 1.5 m octaves) where
  the lace is filtered away. Foam albedo 0.7; thin foam tinted by 1.4 m of the water's absorption.
- **Windrows** (old foam lines along the wind, from 13 m/s) and **gale streaks** (`resolve.wgsl` `streaks`, cascade 1
  only): old foam advected at 3.5 % of the wind speed and gathered into two meandering Langmuir convergence sets 18 and
  29 m apart, decaying over 30 s, shown in world-laid patches 170 × 55 m.

## 5. Optional extensions (briefly)

| Feature | Files | Idea |
| --- | --- | --- |
| Dispersive wake field | `wake/mod.rs`, `wake/kernel.rs`, `wake_field.wgsl` | iWave-like height field on a movable anchor: `∂²h/∂t² + γ∂h/∂t = −g√(−∇²)(h + p/ρg)`, √(−∇²) as fitted kernels on a reduce/collapse pyramid, leapfrog at 30 Hz, sponge edges, hull pressure as swept Gaussians. Display texture (h, ∂h/∂x, ∂h/∂z, foam) |
| Trail foam and slick | `trail.rs`, `stamps.rs`, `stamps.wgsl` | Per-ship samples painted as max-blended quads into a 2048² `Rgba8Unorm` atlas of 8 × 8 tiles: churn, slick, and where each point lies across its band (for streaks along the track). Slick stills cascades' waves shorter than 25 m |
| Bow waves | `bow_waves.rs`, `wake.wgsl` `bow_waves` | Analytic, per hull frame, nearest 8 hulls: a crest climbing the stem (Noblesse et al. 2008), the Kelvin V, white water |
| Hull contact foam, shelter | `hull_foam.rs`, `hull_profile.rs`, `wake.wgsl:856` | Waterline profile per hull; foam along it; hull sides occlude sky and light for the water beside them |
| Torpedo tracks | `torpedo_tracks.rs` | 1536 m view-anchored field, 1024² `R8Unorm` |
| Coasts and surf | `shore.rs`, `waves.wgsl` `shore_at`, `surf` | Per cascade, keep `exp(−5/8 λ² (λp(F)⁻² − λp(F_open)⁻²))` of amplitude at Saville's effective fetch (7 bearings within 45°, cos²); swell shadowed by land, refilling over 15 km; wave height ≤ 0.42 × depth; surf bores at √(g d) once a swell period |
| Lakes | `lakes.rs`, `lake.wgsl` | Flat meshes at their own levels, the sea's optics, Cox–Munk ripples by fetch, occlusion decided in the fragment from the opaque depth |
| Rain on water, spindrift, spray haze | `rain.wgsl`, `spindrift.rs`, `spray_haze.wgsl` | Capillary rings in the normal or as slope variance; GPU-only sprite slots on world cells from 13 m/s; exponential spray haze from 17 m/s |
| Caustics | `caustics.rs`, `caustics.wgsl` | Light gain `1/|det(I + s H)|`, s = d (1 − 1/n), H the Hessian of h from the derivative maps; layers per depth from 2 m × 2.5 |
| Underwater | `underwater.rs`, `underwater.wgsl`, `underwater_ray.wgsl` | Probe per pixel, scene × exp(−c d) to the first surface, closed-form in-scatter, waterline film, motes for motion cues |

## 6. Couplings with the sky and clouds

What the ocean consumes:

| From | What | Where it binds |
| --- | --- | --- |
| Sky (`sky/shaders/environment.wgsl`, `sky/src/environment.rs`) | Environment probe: `Rgba16Float` atlas in three.js's PMREM CubeUV layout (sharp levels down to 16² faces, then 6 extra blur levels at σ 0.125–0.582 rad); `SkyEnvironment` uniform (80 bytes): light direction, intensity, colour, night flag, atlas size. `env_radiance(dir, roughness)` and `env_irradiance(n)` | group 3. The ocean carries a byte-for-byte mirror (`ocean/shaders/sky_environment.wgsl`, `ocean/src/environment.rs`) and a stand-in filler for its stills; use the sky's real one |
| Sky (renderer) | Aerial perspective volume (`naval::aerial`, `aerial_apply`): atmosphere + weather haze along each ray | group 0, bindings 6–7 |
| Clouds (`clouds/shaders/shadow.wgsl`) | Cloud shadow map: transmittance of the celestial light at a world point, multiplied into the direct light only (the sky term already holds the cloud-filtered light) | group 1, 11–13 |
| Clouds / weather | Rain amount 0–1 (`OceanSurface::rain`), lightning flash for spray | uniforms |
| Renderer | Sun shadow maps (`water_sun_shadow`), point light list (`naval::lights`), camera with Earth curvature, opaque colour/depth copies | groups 0, 1 |

What it gives back: the underwater probe (the clouds draw nothing where a ray starts under the waves), sea heights for
the clouds' rain splashes (`Ocean::rain_heights`), caustics and hull height patches for the ships' shader.

## 7. Lessons and pitfalls from the dev logs

Each item names the `docs/` section where the measurements are.

1. **Tile repetition shows in sun glitter from above, not elsewhere.** Hex tiling of the middle cascade (tried first)
   was reverted: it cost 0.2–0.6 ms and the owner still saw repeats. The fix was stochastic tiling of *both* coarse
   cascades. The earlier test passed because it checked the field, not the shaded surface; `tests/tiling.rs` now
   renders the full sunlit surface and requires |correlation at the tile period| < 0.15 (1024 m went 0.208 → 0.008)
   with slope energy within 15 %. ("FFT patch repetition in sun glitter")
2. **Two swell sines read as a ruled corrugation**, even with noise on amplitude and phase: noise cells laid along
   crests show as blocks. A spread of components within ±46° fixed it; ±29° still drew lines from 900 m up. Judged on
   stills from 150–1200 m with each row's mean removed and contrast stretched. ("The open sea's swell is short-crested")
3. **Whitecaps were round discs 20–60 m across**: the dominant cascade broke and foam diffused isotropically at
   3 m²/s. Breaking was moved to short waves (1/32–0.3 of the mean wavelength, after Kleiss & Melville 2010) and
   spreading cut to 0.015 λ c ≤ 1 m²/s, a quarter across the wind. Coverage (Monahan) was kept: 9 m/s from 700 m up is
   about 5.6 % white. ("Whitecaps since the port")
4. **Foam looked like white cut-outs and ribbons**: pure white albedo, hard thresholds, and the lace vanishing with
   distance. Fixes: albedo 0.7, thin foam tinted by the water, torn core edges, a world-space tear for distant pixels,
   streaks as lace. ("Foam that reads as foam")
5. **Glitter from aircraft looked like brushed metal**: the per-pixel Cox–Munk mean is right on average but a real
   pixel at 600–2500 m holds 0 to a few sun images. Poisson glints keep the mean in linear light. Cap each glint (1.0 ×
   sun): uncapped, one glint came out 100× the knee and bloom turned it into a soft halo. Centimetre pixels still draw
   the mean. ("The sun's glitter as glints")
6. **Brown/rust speckle around hulls came from SSR**: rays from troughs hit the hull's anti-fouling *below* the drawn
   waterline in the opaque copy, and the roughness smear slid along the screen onto the deck. Reject hits below the sea
   height at the struck point; turn the smear in the ray's vertical plane and require the copy's surface where that
   ray meets the wall. ("Reflections without hull bottoms or decks")
7. **Warm specks on the open sea are glitter, not a bug**: widening facet variance turns them into a tan sheen;
   culling pixels with few facets darkens the glitter path. Sparse world-anchored glints were the answer.
8. **A fixed-resolution clipmap was too coarse beside hulls** (4 m facets; every wave under ~16 m was normal-only).
   Choose the finest level from the view's standoff and morph it out. About half the added cost is vertices and half
   the shading of small triangles; thin sliver triangles cost because each shades whole 2 × 2 quads. A square-cell fan
   to the horizon added 3.6 ms at 8×; rows a fixed pixel height beyond a knee brought that to +1.2–1.5 ms. ("The water mesh
   follows the view's standoff", "A magnified view draws a fan")
9. **Objects must read the sea as drawn, not the full field.** A hull far from the mesh centre, reading full-detail
   heights while the mesh there was flat, lit her plating as submerged: black strips along the waterline through
   binoculars. Use `drawn_height_at` with the local mesh spacing. ("The clipmap follows a magnified view")
10. **Draw the simulation's sea where ships float.** A client-only swell would move water under hulls that do not
    move; adding it to the simulation would change gunnery and flooding. ("The swell: the sea the hulls ride")
11. **The shader is register-bound.** A flat-coloured sea costs 0.12 ms; nearly all cost is the fragment. Code that
    never runs still costs ~0.1 ms; SSR costs ~1 ms on Apple GPUs just by being compiled in. Hence the separate
    `POINT_LIGHTS` pipeline and SSR tile skipping (water pass 1.0–1.8 → 0.7–1.3 ms on an RTX 5070 Ti). Apple M5 Pro at
    2592 × 1545: 3.8–6.1 ms for the sea; per-stage breakdown in "The sea's GPU cost on High". Render-pass timestamps
    read zero on that tiled GPU; measure by A/B of shader variants.
12. **Tile SSR by reach**: land drawn past the far plane put a band in every ray's screen box; mark only surfaces a
    ray can reach (≤ 60 km). Saved 0.4–0.9 ms from a ship's bridge.
13. **Point lights**: kneeing each light's glints at its own irradiance made a fire's glitter path 30–40× too dim;
    use the sun/moon knee with a floor. Unshadowed lights mirrored fires through hulls; march toward a point 5 m above
    the light over the depth copy, 16 steps, only where the light adds > 3 % of the pixel. ("The scene's point lights")
14. **Time compression**: running waves at 64× made the land seem to slide through the water. The sea runs at most 2×
    real time; beyond that the hulls are let go of the drawn sea. ("Coasts, and the sea under time compression")
15. **f32 phase drift**: quantise ω to multiples of 2π/T and wrap time at T = 4096 s.
16. **Distant ships cast no shadow on the sea** when only the near/wide shadow maps were read; sample the per-ship
    view maps too, taking only hulls' shadows from them. ("Distant ships' shadows on the sea")
17. **Lakes**: sea crests drawn under the land poked through shallow salt ponds. The lake pulls its depth toward the
    eye past the sea's crests and decides occlusion in the fragment from the opaque depth. ("Lakes and ponds")
18. **Underwater**: a bright underside and kilometre visibility (the old web look) read as fake. Use Snell's window
    plus total internal reflection of the deep, and the map's absorption as beam attenuation (40–65 m in blue).
    (`docs/sea-sky.md`, "The underwater view")
19. **Temporal AA**: the sea keeps 70 % history; full history smears foam and rain rings, none makes glints dance.
20. **Storms read as chop on a gentle swell**: a fetch-limited 5.5 m sea at 18 m/s put most height in short steep
    waves. Left as is; two sines never exceed 0.93 Hs, while a random sea's highest of a few hundred is ~1.8 Hs. A
    re-implementer wanting heavy weather needs wave groups in the long swell. ("The swell", "Left as it is")

## 8. Reading order

1. `shared/render/src/frame.rs` (`Phase` `:98`, `SceneLayer` `:345`): the frame contract every pass hangs on.
2. `ocean/src/ocean.rs` (`prepare` `:650`, `draw`, `after_copies`): the per-frame pass order.
3. `ocean/src/lib.rs`: tiers, cascade sizes, parameter structs.
4. `ocean/src/spectrum.rs` (`build_spectrum` `:261`) and `ocean/src/sea_state.rs`: the spectrum and the realistic sea.
5. `ocean/src/field.rs` (`WaveUniforms` `:174`, `rebuild_spectrum` `:1166`, `update_with` `:1224`): what goes to the GPU.
6. `ocean/shaders/fft.wgsl`, `ocean/shaders/resolve.wgsl`: the FFT layout, the three fields, whitecap injection.
7. `ocean/src/whitecaps.rs` (`cascade_breaking` `:169`, `set_breaking_thresholds`): breaking statistics.
8. `ocean/shaders/waves.wgsl` (`scatter` `:107`, `displacement` `:487`, `height_at` `:565`): reading the field.
9. `ocean/src/geometry.rs` and the vertex stage of `ocean/shaders/surface.wgsl` (`:176`): clipmap, morph, ring, fan.
10. `ocean/shaders/surface.wgsl` (`wave_surface` `:312`, `fragment` `:486`): the shading graph end to end.
11. `ocean/shaders/reflection.wgsl` and `ocean/shaders/water_body.wgsl`: Fresnel, lobe, sky taps, glitter, glints,
    water colour.
12. `ocean/shaders/foam.wgsl` and `ocean/src/foam_texture.rs`: foam layers and the equalised texture.

Then as needed: `ocean/shaders/screen_reflection.wgsl`, `ocean/src/hull_sea.rs`, `ocean/shaders/wake.wgsl`,
`ocean/src/underwater.rs`. The GPU tests are specifications with thresholds: `tests/field.rs` (FFT vs DFT, mips),
`tests/tiling.rs` (no visible repeats, energy kept), `tests/surface.rs` (every tier draws; the fan covers the frame),
`tests/hull_heights.rs`, `tests/wake.rs`, `tests/caustics.rs`.

## 9. Game-specific parts you can ignore

- **Parity with the removed web game**: `examples/stills.rs`'s web comparison, "How it matches", tests pinning
  `mode_hash` and spectra to TypeScript output, three.js conventions kept for parity (the PMREM layout and its measured
  blur table, `mx_noise.wgsl`). Any prefiltered environment map and any noise will do.
- **Calibration data**: the wind table, map waters, port/battle fog values (`calibration.rs`), the geographic water
  atlas (`optics.rs` `WaterField`). Keep the idea (wind → Hs, λp; absorption per water type), choose your own numbers.
- **The simulation coupling**: `hull_sea.rs`'s per-hull slots, `racing`, `hull_rate`, the theatre's clock rules,
  `focus` on coupled hulls. Keep only if your physics owns a separate sea.
- **Combat effects**: the point light list, torpedo tracks, impulses from shell strikes (`wake/impulses.rs`),
  spindrift's lightning flash.
- **Theatre/world plumbing**: `ShoreMap` building, lake grids from a region bake and the satellite photo tint, the
  480 km horizon ring for land drawn beyond a window, render-origin offsets.
- **Renderer plumbing**: naga_oil composition (`shaders.rs`), the pipeline registry and hot reload (`new_in`,
  `adopt_pipelines`), staging belts, GPU timers, the packed wake uniform (forced by WebGPU's four guaranteed bind
  groups and per-stage limits), the `StandInSky`/`StandInEnvironment` used only for stills.
- **Developer tooling references** in the docs (`bun native/tools/...`, `.build/...` captures, PR numbers).
