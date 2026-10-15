# Ocean lane (`native/crates/naval-ocean`)

Port of the game's own ocean (`src/game/ocean`, see its README) to wgpu: the seeded spectrum, the GPU
FFT cascades with whitecap foam, the clipmap surface and its physical shading. Portable crate (passes
`cargo check -p naval-ocean --target wasm32-unknown-unknown`); the stills example and GPU tests are
native-only. Outputs go to `.build/native/ocean/`.

Status (2026-09-30): the open sea is done and matches the web frame for frame at 9, 15 and 25 m/s from
bridge, chase and overhead cameras and into the sun (numbers below). `Ocean` is a
`naval_render::SceneLayer` and the stills render through the renderer core. The ship wakes (wake field,
trail foam and slick, torpedo tracks, bow waves, hull contact foam and hull shelter) are ported by the
water-effects lane (`native/docs/water-effects.md`, "Wakes" below), the sea around hulls by the sea-sky lane
(`native/docs/sea-sky.md`), and so is the underwater view.

The seabed the sea shows through its shallows is the land layer's, read from the opaque scene. Since 2026-10-05 the land
shades it, more than 3 m down, as the seabed alone (sand darkening with depth, rock on the steep, no relief:
native/docs/terrain.md, "The land's shading cost"), not through the land's whole surface; nothing in the sea changed.

## Coasts, and the sea under time compression (2026-10-08)

Near land the theatre drew the open ocean's sea: the same swell and whitecaps ran up to every beach, lee or not, and
under time compression the waves ran at the world's speed (64 times as fast at 64×), so the land seemed to slide
through the water. Now:

- **The coast** (`naval_ocean::shore`; `waves.wgsl`'s `shore_at`, `surf`). The theatre builds a `ShoreMap` of the
  sea's land window on a thread of its own (`naval-client`'s `theatre/shore.rs`), again when the window is cut afresh
  or the wind turns 4°: the land's heights (40 m) and, on a 160 m grid, the open water upwind for the wind sea
  (Saville's effective fetch, seven bearings within 45°, cos²) and for the swell (±10°). A beach sample holds the
  sea's fetch that runs onto it, so the water between it and the last sea sample is not read as sheltered. Each
  cascade keeps the share of its amplitude the JONSWAP fetch law leaves at that fetch (`band_fetch_table`, tabled
  over 16 fetches from 50 m to 100 km): in a lee the long waves are not raised and the short ones stand. The swell
  is gone in the land's shadow and fills back in over 15 km. No wave stands higher than 0.42 of the depth. The far
  whitecap share takes the energy the cascades keep.
- **The surf.** Where the water is shallower than the arriving waves break in (their height ÷ 0.42), bores run in
  once a swell period at √(g·d); over the seabed's slope `s` one takes 2·√d / (s·√g) from depth `d` to the
  waterline, so the lines lie a shallow-water wavelength apart: several on a gentle beach, less than one on a steep
  one (a white band that runs in and drains). They are their own foam layer (`WaveSample::breakers`), drawn as
  their mean where the pixel is too large for the lines or their tear.
- **The clock.** The theatre draws its sea at no more than twice real time (`theatre::SEA_RATE_MOST`; the FFT and the
  swell's spread run on the ocean's own clock). Above that (`Ocean::hull_rate` beyond `time_scale`) the hulls are let
  go over half a second of wall time (`Ocean::racing`): the combat sines' energy moves into the spread, which is drawn
  everywhere, and the hulls ride the world's sea loose of the drawn one.

Hulls ride the simulation's sea, which knows no coast, so within a hull's coupling the swell is kept whole.

Measured on the "Japan" save (7 m/s toward +x, Hs 1.2 m): a beach 20 km downwind keeps a 43 km fetch to its waterline
(cascades 0.40/0.85/1.0, swell 0.90, 1.28 m arriving, surf inside 3 m of water: 13 m of a 1:4 shore); a beach in the
lee keeps 2.7 km and 0.2 km (0.0/0.12/0.80, swell 0.01, 0.14 m). The mean frame-to-frame change of the sea's pixels
(0–255, 48 frames at 60 Hz) at 64× fell from 10.2 to 2.8 from the sea and from 15.2 to 3.5 beside the surf (2.4 at
1×). The harness's `water` returns the coast at its `sample` (`coast`: depth, both fetches, scales, arriving) and
the clocks (`sea_rate`, `hull_rate`, `racing`).

Not done: the simulation's sea near land (a hull in a calm lee still rides the open swell, and draws it about her);
refraction (crests do not turn to the shore); caustics and spindrift take no coast; battle scenes have none.

## The sun's glitter as glints (2026-10-08)

From an aircraft (600–2,500 m, into the sun) the glitter path was an even grey sheen, like brushed metal: each pixel
drew `reflection::sun_glitter`, the Cox–Munk mean over its facets, where real glitter from the air is a sparse field of
sun images, none to a few glinting wavelets a pixel, each far brighter than white. The surface now draws that mean as
glints (`reflection::glitter_glints`, called in `surface.wgsl`):

- **The count**: a pixel expects `glitter_density` (the slope density at the half vector, the mean's own Gaussian) ×
  `GLINT_DISC_SLOPES` (the slope area that mirrors part of the 1.4° sun disc, 1.17e-4) × `GLINT_FACETS` (400 a square
  metre) × its area on the water (from the grid's screen derivatives) glinting facets. It draws a Poisson count about
  that (inverse CDF up to ten, Gaussian above) and scales the mean by the count over the expected number, so the mean
  over many pixels is kept in linear light.
- **Where**: counts are drawn on cells laid on the world about a pixel across, the two power-of-two sizes about the
  pixel blended (one rounded size drew horizontal seams where it changed). Below 0.1 m pixels the mean stands, the
  waves the pixel resolves drawing the glints; the glints take over by 0.4 m.
- **When**: each cell draws again four times a second (`GLINT_TWINKLE`) on its own phase, held for 60 % of the slot
  and faded into the next, on the waves' clock.
- **How bright**: the mean keeps its knee (`GLITTER_KNEE`, 0.25 × the sun's intensity) and one glint's pixel is held
  to `GLINT_PEAK` (1.0 × the sun's intensity, `reflection::luminance_ceiling`). Without the cap a lone glint 100–400 m
  from a bridge came out a hundred times the knee, and the bloom spread it into a round, soft halo.

North Atlantic, 9 m/s, 15:30, looking into the sun (luma 1st/10th/50th/90th/99.9th percentiles of a patch in the near
glitter path, 1600 × 1200): from 1,200 m 89/120/152/168/178 before, 35/74/138/174/202 after; from 600 m
104/134/157/170/179 before, 28/67/128/176/210 after. From 2,500 m, where a pixel holds hundreds of glinting facets, the
sheen barely changes. From the bridge (25 m) and the deck (8 m) the far glitter path sparkles and the nearest waves
draw the mean. The old note under "Reflections without hull bottoms or decks" (sparse sparkles left out for their temporal
aliasing) is what this does for the far path; centimetre pixels still draw the mean.

Not checked: the twinkle in motion under the temporal filter (the sea keeps 70 % history), the GPU cost against the old sheen (a
300 m air view draws in 9.9–10.1 ms a frame on the dev client), and other facet densities or a looser knee (the
brightest glints stop near 220 of 255).

## Geographic water (2026-10-07)

Theatre no longer inherits one North Atlantic water body everywhere. `naval-client`'s
`theatre/water.rs` selects representative offshore optics and an authored atlas of broad coastal waters, bays and
river plumes. An arbitrary editor region uses a smooth latitude-based offshore fallback, with a clear Mediterranean
override; latitude is a visual fallback, not a water-colour measurement. The atlas's positions, sizes and strengths
are approximate, not historical observations. It has no seasonal biology or moving sediment simulation.

`src/optics.rs` owns five profiles (`clear`, `oceanic`, `productive`, `sediment`, `organic`): absorption and
backscatter coefficients per metre, at approximately 620, 550 and 460 nm. `WaterField` holds a baseline and up to
twelve elliptical areas; broad waters blend first, local plumes last. Each has a uniform middle and a smooth outer
edge. The host projects geographic centres into the persistent scenario frame, then subtracts the render origin in
f64 before sending them to the GPU. The shader samples the coefficients at each water point, so different waters
can appear in one view and do not move with the camera. Distant atlas entries are culled in the scenario frame.

The surface, underwater composite and motes share `shaders/water_properties.wgsl`. Colour comes from the existing
sun/sky upwelling model; the seabed still supplies shallow-water colour. Geographic transparency uses a fixed
8:1 scattering/backscatter approximation. The underwater view retains its approximate illumination, adjusted by
the same optical reflectance ratio, and samples visible paths at their midpoint (capped at 100 m); this is suitable
for kilometre-scale blend widths, not detailed volume transport. Submerged hull lighting takes the water near the
camera. Weather changes light and waves without changing the geographic field. Developer map fixtures keep their
existing optics unless the harness's `water` command replaces them.

`bun native/tools/water-atlas.ts` captures all profiles with the same ship, camera, wind and noon lighting, a plume
transition, clear/murky underwater views and both built-in theatres. Outputs and diagnostics go to ignored
`.build/native/water-atlas/`; `--no-build` reuses the current client. It also checks that malformed areas leave the
live field unchanged and that water varies across a plume. Unit tests cover boundaries, invalid inputs, capacity,
theatre recentering and longitude wrapping; the ocean's shader composition test covers the shared GPU layout.

## Lakes and ponds (2026-10-05)

Lane C of the terrain programme, branch `goldflag/lake-surfaces`. A theatre window's lakes, ponds, reservoirs and salt
ponds (the region bake's lake layer: native/docs/theatre.md, "Lakes for the water's renderer") drew as low flat ground;
now they are water at their own levels, a layer of their own in this crate beside the sea (`src/lakes.rs`,
`shaders/lake.wgsl`), drawn in `Phase::Water` with the opaque copies, so it shades with the sea's own optics. The host
and where it is wired: native/docs/theatre.md, "Lakes as water". Presentation only.

```rust
let mut lakes = LakeLayer::new(&mut ctx, &LakeGrid { origin, cell, columns, rows, levels }, settings,
    &environment.bind_group_layout, &environment.bind_group)?;       // naval_ocean::lakes
lakes.set_offset(land.offset());          // world = chart + offset, as the land: every frame
lakes.set_cloud_shadow(device, Some((&map, &sampler, &uniforms)));  lakes.set_wind(speed, direction);
lakes.set_water_shadows(quality);  lakes.set_sea_crest(0.75 * hs);  lakes.set_range(2.0 * visibility);
lakes.set_photo(Some(LakePhoto { view, origin, extent_m, texels, haze, version }));   // the land's photograph
lakes.set_shown(false);  lakes.counts();
```

- **The mesh** (`LakeMesh::build`): each lake (samples 8-connected at one level) covers its samples and every square
  between four samples with a corner in it: from its samples out to their neighbours', whose drawn land the bake raised
  0.3 m above the level, so the land crossing the level draws the outline and the mesh never reaches land it left low. A
  square two lakes share is the higher one's. The squares are merged into quads of at most 16 cells a side (1.1 km: the
  Earth's curve, applied per vertex, sags 2.5 cm under one) within chunks of 32 cells, the unit the layer culls: outside
  the view (the boxes lowered by the curve's drop at their far corner) and past `set_range` (twice the scene's
  visibility, where its air leaves under a thousandth of the contrast). California's sea window off San Mateo: 219 lakes
  in 5,108 triangles and 210 chunks, built in 7 ms; an air window over the bay 837 lakes in 13,550 triangles.
- **The mask**: the lakes' samples as an `R8Unorm` texture (a texel of nothing about it): 1 at a lake's samples, the
  land beside them a 3 × 3 binomial blur of them (0.25 beside a straight shore; the plain samples' bilinear contour
  stepped round the grid's corners), read filtered. The water is cut at its contour (0.30 near the camera, about 65 m
  past the last sample, softened over 0.06), which stops it spilling over land lower than the lake beyond its raised
  shore.
- **Where the land is coarse** (`mask_led`): the land crosses the level at its own resolution, which far off (its
  level-of-detail ranges as `naval_terrain::lod::base_range` lays them out) or in an air window (140 m samples, every
  other one of the lakes' grid) is coarser than the lakes'. There the mask draws the outline (contour 0.42: where the
  land would cross for a bed 1 m down and a shore 0.3 m up, 0.77 of the way to the next sample), the water is taken 3 m
  deep toward a lake's middle for its colour, and the land within the outline up to 10 m above the water no longer hides
  it.
- **What hides it is the fragment's, not the depth test's.** The sea's waves are drawn everywhere, under the land too,
  and their crests stood through the bay's salt ponds (beds at 0.25 m) as dark foaming blots. The water is pulled toward
  the eye along its own ray (it covers the same pixels) past the sea's crests (`set_sea_crest`, 0.75 of the drawn
  significant height) and, where the mask leads, past the coarse land; the fragment then reads the opaque depth (the
  land alone: the sea draws in this phase) and discards where the land stands in front of the true water (by more than 5
  cm, or 10 m where the mask leads), and where the surface behind lies below 0.1 m: that is the sea's floor, not a
  lake's bed (the bake keeps every bed 0.25 m up), so a lagoon's squares reaching over the sea beside it draw nothing
  there.
- **Shading** (the sea's modules: `reflection` and `water_body`), only for what survives the cuts above, which come
  first (the stage takes no screen-space derivative, so it can discard early; the pixel's footprint is the sea's
  analytic one): the environment (clouds included) mirrored about the resolved normal in one tap blurred by the
  unresolved facets' spread, with their mean Fresnel (a lake's mirror is soft; the sea's visible-facet lobe and three
  taps cost more than they showed here), the sun's glitter kneed as the sea's (evaluated only where the facets can turn
  the sun to the eye, within √40 deviations), the sun maps' shadows (`water_sun_shadow`, at most the Low water-shadow
  filter: what falls on a lake is mostly the land's and the trees', soft at the maps' texels) and the clouds', the water
  body over the bed by the opaque copies (`column_transmittance`, `water_body`), the scene's air (`aerial_apply`), and
  night as the sea takes it (the environment's light is the moon's). Ripples: Cox and Munk's slope variance for the
  sea's wind, calmed (0.6, a lake's films and banks) and by a short fetch (√(fetch / 10 km), at least a quarter), in
  drifting patches (cat's paws: value noise 170 × 90 m, its lattice turned off the wind's axes so no cell shows as a
  block, a gain of 0.45 to 1.4); four gravity-capillary wavelets (0.37 to 2.4 m about the wind's heading, their
  constants worked out ahead: a cosine a wavelet a pixel) carry 60 % of it where a pixel holds a fifth of their
  wavelength or less, the rest the unresolved facets'. The water's colour: fresh water's absorption (pure water's with a
  lake's dissolved organic matter: 0.31, 0.17, 0.52 /m at 620, 550, 460 nm) and silt (backscatter 0.010-0.013 /m),
  brown-green; a lake at or below 2 m (a lagoon or salt pond) clearer and milkier (0.30, 0.09, 0.12 and 0.032-0.042);
  and where the window's satellite photograph covers a lake's middle (the mask above 0.55) its own colour, read two
  levels coarser than the pixel asks: the photograph's reflectance less its haze and the glare it held, lit by this sky,
  85 % of the deep water's light. The salt ponds come out red, pink, green and white as the photograph has them.
- **The shore**: where the water over the bed thins to nothing (0.25 m straight down, from the opaque depth) it fades
  into the opaque copy, so the land's level-of-detail morph moving its crossing shows no seam and no z-fighting (the
  crossing is where the water is already clear); a hint of foam where a breeze (from 2 m/s) laps the shore (High).
- **Medium** shades with two wavelets and no foam (`LakeQuality::Medium`, the terrain setting's); Low draws the land on
  Medium and the lakes with it.
- **Motion**: 70 % history in the temporal anti-aliasing, as the sea (`draw_motion`).

**Cost**: native/docs/theatre.md, "Lakes as water". **Checked**: `cargo test -p naval-ocean --lib lakes` (a sample's
squares; two lakes leave the land between them dry; a big lake's quads cover its squares once, within their chunks; the
higher lake takes a shared square; culling by view, range and the land's offset; a far pond the curve drops into view)
and `--test all -- lakes::` on the GPU (water where a round lake's samples are and nothing outside them; placed by the offset;
land a metre above the level hides it, but not where the mask leads; Medium draws it; none over the sea's floor;
`.build/native/ocean/lake-gpu-test.png`).

**Open.** A lake's surface is one level: a river's run of ponds steps from one to the next where their samples meet. No
reflection of the land and hulls about a lake (the sea's screen-space reflections are the sea's; the lake mirrors the
sky). No point lights on lakes. Frozen lakes in winter are drawn open. Where the band's 35 m outline of a lake reaches
past its 70 m samples' contour (a narrow arm), the arm is a bed with no water. The depth a lake writes is the pulled
one: in a gale, whose crests stand over a low lagoon, it is up to (crest - level) / sin(elevation) nearer the eye (at
most half the distance), and a transparent effect within that of the water on screen (spray over a lagoon's bar) is
hidden behind it.

## Distant ships' shadows on the sea (2026-10-05)

The sea took the ships' shadows from the sun's near and wide maps only (the web game's `updateWaterShadows`, which left
the view maps off for want of a sampler), so a hull beyond the wide map (380 m across the sun's bearing from the ship
the maps follow) shaded her own decks from a view map but threw no shadow on the water, and on the Medium preset, whose
sun shadows are Low, she had none at all (no view maps under 2048 texels). Now `naval::shadow::water_sun_shadow` reads
the view maps too, by the weights the ships' paint uses, and Low draws its view maps at 1024; the surface and the spray
(`spindrift.wgsl`) take it unchanged. The sea needs no binding for them (they are layers of the near map's array) and
casts into no map, so it cannot shade itself however the waves move it. From a view map it takes only the shadows of
the hulls the map was fitted to, not the land's that is drawn into it too (which would lie on the water in her square
alone, ending in straight edges across the glitter). Why each choice, the filters per water-shadow quality and the
offsets: `render-core.md`, "Sun shadows", "What the ocean needs" and "Only the hulls' shadows from the view maps".
The water pass's cost (M5 Pro, 1920 × 1080, a loaded machine): about +0.05–0.10 ms on the Medium preset and +0.05–0.28 ms on High (`render-core.md`, "What the view maps on the
sea and on Low cost").

The Low preset's water shadows are off, so on Low the sea shows no ship's shadow at all, near or far: a setting
choice, left as it is.

## Whitecaps since the port (2026-10-03)

The browser game's whitecaps drew round white discs 20–60 m across: the longest cascade, holding the peak, broke, its foam
spread 3 m²/s into a round patch, and a pixel too coarse for the lace drew the patch's mean, so the disc's outline was
the spread's. Native-only changes since, measured against the stills of `.build/ocean-review/` (no web reference
remains):

- **Breaking window** (`whitecaps.rs`): waves break from 1/32 to 0.3 of the sea's mean wavelength, fading out by 0.6
  (was 1 and 2). Breaking crest length peaks at a third to half of the peak's phase speed (Phillips' Λ(c), Kleiss &
  Melville 2010), so the dominant waves rarely break: on High at 9 m/s the 1,024 m cascade carries no breaking, and the
  breakers are 4–8 m waves (periods 2.2 and 1.5 s against 4.3 and 2.6). The thresholds still hold Monahan's coverage.
- **Spreading** (`field.rs`): 0.015 · λ · c, at most 1 m²/s, a quarter of it across the wind (was 0.05, 3, a half), so a
  thin crest stays thin.
- **Shading** (`foam.wgsl` `whitecap_foam`): a coarse pixel draws the same whitecap on the lace's coarse layout instead of
  its spread mean (no more `spread_whitecaps`, the surface's `foam_mean` read is gone), the core starts at 0.85–1.3 of
  fresh foam and covers 85 % of it, the old patch's lace covers 20–52 % at 10–50 % opacity and leans further into the
  wind's streaks. A breaking face stands up 0.4 m (`WHITECAP_FACE`), so a grazing pixel keeps its core as a short
  fleck. The bubble cloud under whitecaps is read over 4 × 1.5 m (was 8 × 2.5) from 0.12 to 0.7 of the bubbles, at 0.2 of
  its strength (was 0.45): it drew the pale pills that outlived each disc.

Share of sea pixels white (brighter than their surroundings by 35 levels), before → after, from 700 m up and from 60 m:
9 m/s 5.7 → 5.6 % and 0.55 → 0.42 %, 13 m/s 5.0 → 4.8 % and 1.4 → 1.1 %, 17 m/s 4.5 → 3.6 % (from 140 m 4.0 → 2.9 %).

## Foam that reads as foam (2026-10-04)

Renders of master showed whitecaps as hard-edged, uniformly white cut-outs at mid distance and in a storm, gale streaks
as smooth white ribbons and a crisp white stripe along every hull's waterline. Changes (`foam.wgsl`, the foam part of
`surface.wgsl`):

- **Whitecaps** (`whitecap_foam`): the core thickens from 0.45 to 1.4 of fresh foam (was 0.85–1.3), its outline torn by
  the churn and the lace together (`CORE_TORN`) and its outer flecks 0.4 opaque. Where the lace is filtered away (a
  grazing or distant pixel) the texture's sway goes, so a world-anchored value-noise tear (`foam_tear`, octaves of 9, 3.7
  and 1.5 m fading with the pixel) scales the amounts and the residue's cover, and the residue thins over the foam read
  across the bubble cloud's ellipse (`WaveSample::foam_soft`, the spread read the surface already took) instead of at
  the field's last texel. Near, where the lace resolves, its own threshold draws the edge as before.
- **Colour**: `FOAM_ALBEDO` 0.7 (dense sea foam reflects 0.55–0.75), deeper crevices (`CREASE_LIGHT` 0.62), and thin
  foam takes the water's colour (`thin_foam`: 1.4 m of the water's absorption, 75 %), so lace and windrows are bluish
  grey and only the dense core is near white.
- **Finer lace**: the lace's own finer copy (4.7× finer, turned 1.1 rad) moves every threshold ±0.15 where a pixel
  resolves it, so edges fray at centimetres.
- **Gale streaks** (`gale_streaks`): lace, not ribbons: cover 0.7 of the streaked lace torn by the round lace (45 %),
  0.3–0.85 opaque with how much old foam there is, breaking along each line (the tear laid six times longer along the
  wind, keeping 35 % where it breaks).
- **Shoreline foam**: torn along the line, thinning away from the contact (cover 0.4 at most) and tinted as thin foam;
  against a hull's side (where its shelter hides the sky) 85 % of it goes, the hull's contact foam (`wake.wgsl`
  `hull_foam`) drawing the white water there.

In a storm (18 m/s, a Bismarck at 7.7 m/s, from 140 m at 4°, paired with master's foam shaders in one client) the share
of sea pixels 35 levels brighter than their surroundings went 7.2 → 6.7 %; at 13 m/s close up, whitecaps keep a bright
core inside bluish, see-through lace. The bubble cloud under foam is paler (`BUBBLE_PATH` 2.6 m, was 4.5;
`BUBBLE_STRENGTH` 0.6). Before and after stills: `.build/native/foam/` (`final-before-*`, `final-after-*`).

## The swell: the sea the hulls ride, everywhere (2026-10-04)

The drawn sea was a pure wind sea whose longest, highest waves repeated on the first cascade's tile, and the long swell
combat poses hulls on (naval-sim's `SeaState`: two long-crested sines, 0.7 of the amplitude at four times the calibrated
peak wavelength along the wind, 0.3 at 0.57 of it turned 0.8 rad) showed only within a hull length or two of the 16
nearest hulls (`sea-sky.md`, "The sea around hulls"). Now the realistic sea draws that sea everywhere
(`HullSeaUniforms::set_everywhere`, set by `WaveField::couple`; `waves.wgsl` takes the coupling's weight as 1 at every
point): the first cascade's long waves give way to the sines across the whole surface, the shorter waves, foam and
wakes riding on them as beside a hull.

- **Why the sim's sea and not a swell of the client's own**: hulls must sit in the water they ride, and their poses are
  the simulation's. A swell drawn only by the client would move the water under every hull without moving her; one added
  to `SeaState` would change heave, roll, flooding, shells' sea height and gunnery in every battle. The sim's first sine
  already is a swell: 128 m (9.1 s) at 9 m/s with 1.17 m crest to trough, 171 m at 12, 239 m (12.4 s, 3.6 m) at 18,
  where the wind sea's own peak is 70, 100 and 197 m.
- **What it changes**: every hull at any range (not only the nearest 16) sits in the water she rides; the open sea
  carries a long swell to the horizon (short-crested away from the hulls since 2026-10-05, below); the tile's long
  waves, its most visible repeat, are not drawn. The
  drawn significant height is the field's shorter waves with the sines on top, as beside a hull before: 2.4 m at 9 m/s
  (the field alone 1.8), 3.5 at 12, 6.7 at 18, 9.9 at 25 (a wind sea with a swell under it). The art-directed sea is
  drawn as given.
- **Cost** (RTX 5070 Ti, Vulkan, 2560 × 1440, High, `water-cost.ts`, three rounds, with the reflection change below, the
  GPU shared with other sessions): the water pass chase 0.678 → 0.695 ms, bridge 0.485 → 0.478, low 0.431 → 0.419, close
  0.634 → 0.672, overhead 0.689 → 0.675.

**Left as it is: the storm reads as chop.** At 18 m/s the calibration's height (5.5 m, plausible for a fetch-limited sea;
fully developed would be about 8 m) goes to the sines (3.6 m crest to trough over 239 m and 1.5 m over 136 m: steepness
0.015, a swell's) while the field's waves below the split (under about 120 m, near its 197 m peak) keep 3.8 m of height,
steep and sharp-crested: busy chop on a gentle swell. Two sines never make a wave higher than 0.93 Hs (2A) anywhere,
where a random sea's highest of a few hundred is about 1.8 Hs; the groups of big waves are what reads as heavy weather,
and hulls must ride them too, so they belong in naval-sim's sea. Its response there (`SeaState::response` as written,
before the hydrostatics' lag): Bismarck with the sea along her length heaves ±0.23 m and pitches ±0.5° (the 239 m sine is
her own length, so it cancels along her), across it ±1.9 m and rolls ±3.4°; a Fletcher along it pitches ±2.8°. Not
changed: gunnery, flooding and resistance are tuned on that sea.

## Reflections without hull bottoms or decks (2026-10-04)

Tan, rust and brown speckle hugged every hull (a brown halo from the bow quarter, rust grain beside the waterline from
low). It went with screen-space reflections off (`--graphics '{"reflections":"sky"}'`), and debug colours over the trace
showed two sources, both reads of the opaque copy that the mirrored ray cannot see (`shaders/screen_reflection.wgsl`):

- **The hull below the waterline**: a ray from a trough struck the copy's hull bottom (red anti-fouling) under the sea
  drawn at the hull. A read now counts only above the sea where the ray struck (`displacement` at a 0.5 m spacing plus
  `wake_height` there, faded in from 0.05 m below to 0.35 m above).
- **The deck, through the smear**: rough water smears the image over the lobe's spread, and the smear ran along the
  screen, where above a hull side's image lies her deck seen from above. The smear now turns the ray in its vertical
  plane, meeting the struck wall (taken upright) higher or lower, and a read counts only where the copy's surface is
  where that ray meets the wall (within 0.5 m, a tenth of the rise and 0.5 % of the depth). From high views a ship's
  long reflection streak, which was mostly her deck, is shorter; from low ones her side still streaks.

**Not changed: the open sea's warm specks** (a duel's shell camera, no ship near) are the sun's glitter
(`reflection::sun_glitter`, confirmed by colouring it) on near wave faces turned toward the half vector. A pixel draws
the glitter's mean, which on centimetre pixels is a dim, sun-coloured patch, where real glitter there is a few pinpoints
far brighter than white. Widening the facets' spread (a capillary floor on the unresolved variance, a larger footprint
share) spreads the same energy into a tan sheen; dropping the mean where a pixel holds too few glinting facets darkens
the near glitter path into the sun. Sparse, world-anchored sparkles there are the fix, left out for their temporal
aliasing. Since 2026-10-08 pixels wider than 0.1 m draw them ("The sun's glitter as glints"); these centimetre pixels
still draw the mean.

## The scene's point lights on the sea (2026-10-03)

The web's sea took none of the scene's point lights (its material replaced three's lighting): at night a salvo, a
burning ship or a lightning stroke left the water black but for what the screen-space reflections caught of a lit hull.
The sea now takes them (`shaders/point_lights.wgsl`, the loop in `surface.wgsl` beside the sun's terms), each as the sun
or moon lights it:

- **Glints**: the sun's glitter BRDF (`reflection::sun_glitter`: Cox–Munk's slope distribution of the pixel's
  unresolved facets about its resolved normal, Smith masking, Fresnel), with what every light shares taken once a
  pixel (`light_frame`: the facets' frame, their spread, the view's Λ) and a light skipped where the half vector needs
  facets more than √40 deviations steep. The lights' glints sum under the knee the sun's or moon's take
  (`sun_intensity × GLITTER_KNEE`), the sea's ceiling on its glints, never lower than a full moon's (`LAMP_KNEE_FLOOR`:
  with the sun and the moon down the sky's light is zero, and a knee at it would leave the lamps no glints); kneed at
  each light's own irradiance instead, a fire's glitter path 100 m off came out 30–40 × dimmer than the same BRDF
  gives, the forward glitter at grazing views being where its BRDF is largest.
- **Into the water**: the irradiance crossing the mean surface (Fresnel's transmitted share, the cosine of incidence),
  of which the water scatters `subsurface_reflectance` back up, as the sunlit term of `upwelling`.
- **Foam**: the foam's wrapped diffuse light (`foam_radiance`'s share and albedo), whitecaps and wakes about a fire or
  a flash.

A light is a point of three's intensity (its colour × candela in the sea's units), inverse square from 1 m out, with no
cut-off: the reach three's lights carry (126 m for a 16-inch muzzle flash, 70–130 m for a fire) is a culling
convenience, and the sea mirrors a light from wherever it stands. Below `FAINT` (1e-3 of irradiance) a light's diffuse
terms are skipped (a thousandth of the moonlit sea). The host hands `OceanSurface::point_lights` its own list
(`CombatEffects::sea_point_lights` plus lightning's stroke, `naval-client` `scene.rs`): the four flashes, the stroke
and the three most relevant fires in view at any distance (`fires::SEA_FIRE_LIGHTS`), where hulls take the two within
1.5 km; the eight the uniform holds always fit them, so a flash never takes a fire's place on the water.

**What hides a light (2026-10-10).** The lights were unshadowed: a fire on a ship's upperworks laid its glitter path and
glow on the water right up to her side, though her side, her turrets and her upperworks stood between, and a fire on her
far side mirrored on the near water where it could not be seen. Each light is now marched toward over the opaque scene's
depth behind the sea (`light_seen`, the binding the water's column already reads), toward a point `SEEN_RISE` 5 m over
the light: a fire's light stands about half way up its flames, and water that sees their tops mirrors them (marched to
the light itself, flames plainly in view lost their reflections where a deckhouse hid the light's point). The march
starts at the light's end, where the burning ship's own structure stands: sixteen steps placed as the square of their
count, read in batches of four, shifted by a pixel's interleaved gradient noise that moves each frame for the sea's
temporal history to blend (a redrawn frame keeps its frame index, so stills show the noise where a fire is partly hidden),
and stopping 4 m short. A step hides the light where the screen shows a surface in front of it by 0.5 m to the larger of
30 m and a twentieth of its distance (a hull's beam at hand, a hull seen end-on far off); farther behind than that it
passed behind something nearer the eye, which hides nothing. Where the water lies between the eye and the burning ship,
which is where her glitter path lies, the eye's line to any point of the water's path to the light runs above that path,
so a point the screen does not show is inside what hides it. Off the screen's edges and behind the eye nothing hides a
light. A light is marched only where its glints, its light in the water and its light on foam, each weighed by its share
of the pixel (`LightShares`), add more than the larger of 1e-4 (under 3 of 255 levels with the eye at its most open at
night) and 3 % of the pixel's light without the lamps: marched wherever it lit the water, the march cost three times as
much, most of it on water a light lights too faintly for its shadow to show. With the water's shadows off (Graphics; the Low preset) no light is
marched. The fire's glow sprite on the water beneath it (`fires.rs`, `pools`) is not shadowed. Cost (GPU busy, 1728 × 1030, High, the burning Bismarck 120–1200 m off at night,
three fire lights, other lanes building): +0.24–0.45 ms over the unshadowed lights; twelve steps saved 0.03–0.08 ms and
changed up to 20,000 pixels by more than 8 levels.

**Cost.** The lights' block costs the sea's shader registers even where it runs nothing (0.1 ms of the water pass at
2400 × 1350), so the surface is built twice (`POINT_LIGHTS`) and a frame without lights draws the variant without
them: day, and night without combat, draw master's shader. In the lit variant the block runs after the foam, where few
values are live (mid-shader, three fires cost 0.30–0.35 ms; there 0.17–0.22), each term reaching the surface by the
paths the sun's takes (the glints where no foam covers the water, the light in the water through the body's share, the
foam's through each layer's opacity), and a light's glint terms are taken only where it can glint. Apple M5 Pro,
2026-10-03, High, the burning Cleveland 700 m off at night (`.build/native/look-night-light/ab_fold.sh`, 16 alternating
rounds a side, load 6–16): the water pass 4.41 ms without the lights, 4.62 ms with three fires (min 4.38 → 4.57).

## A magnified view draws a fan (2026-10-12)

Moving the clipmap's centre to what a lens is aimed at (the next section) makes the water fine about one point. A lens
high over the sea shows a strip of it kilometres long: at 8× from a foretop 35 m up, from 600 m off to the horizon.
All of that strip but the patch at the aimed hull lay on cells hundreds of metres across, the horizon ring's among
them, where every wave is faded out of the mesh and only shaded: through binoculars the sea was a flat sheet with a
detailed patch at the ship in the sight, and a hull past the focus's 12 km had not even that.

A view magnified 1.25× or more, from an eye 10 m or more above the highest the surface reaches, now draws the sea with
a second mesh in the clipmap's place (`geometry::fan`, `Fan`): a polar lattice about the eye over the wedge of sea
between the frame's edges, from where its steepest line of sight meets the water to where its shallowest does, or
to the sea's horizon and a crest beyond.

- **Cells are the pixels across that the normal lens's are.** Columns stand a step of bearing apart: 1.25 of the least
  a clipmap cell is of its range (two over the cells a level edge holds), over the magnification. On High at 8× that
  is 1/410 of the range: 1.3 m where the frame's lower edge meets the sea, 24 m at 10 km, where the clipmap about the
  eye has 312 m. A coarser tier's cells are as much larger as its lattice's.
- **Rows are square to the knee and a set height on screen beyond it.** Within the knee each row is e^step times the
  range of the one before, so a cell is square. The sea at a grazing angle is foreshortened (a square cell 10 km off at
  8× is a twelfth of a pixel tall), and triangles thinner than a pixel cost their pixels several times over: each
  shades the whole of every two-by-two block it touches. Past the range where a square cell would stand under a pixel
  and a half tall (on High; three pixels on Medium: `FAN_ROW_PIXELS`), rows stand that far apart down the screen
  instead (one over a row's range falls by `Fan::squeeze` a row). From 35 m the knee lies about 500 m off at any lens.
- **The waves are read at the longer of a cell's sides** (`Fan::spacing`), as a level's are at its cell, so the rows
  beyond the knee carry the waves no shorter than they are deep: at 8× full relief over the lower three fifths of the
  sea in the frame, the long swell alone toward the horizon. `wake.wgsl`'s `mesh_spacing` and the bow waves' vertex
  crest read the same spacing (`wake.shading[3].w` set, `wake.field.w` the step, `wake.bow.w` the squeeze), so the
  hull heights' patches hold the sea as drawn under the fan as under the clipmap.
- **Turning or tilting the view moves no vertex.** Columns stand on whole steps of bearing (a whole number of them
  to the turn, so there is no seam where the heading wraps) and rows on whole rows from the knee, so a turn or a tilt
  adds and drops cells at the frame's edges. The lens and the eye's height reach the
  lattice a notch at a time (an eighth of a doubling), so neither's drift moves it. The lattice does travel with the
  eye: a ship under way slides it through the waves, which the waves' own filter (nothing shorter than two cells) keeps
  from showing; a frozen sea seen from an eye moved 1.25 m changes a quarter more under the fan than under the old flat
  mesh, most of either change the parallax of the shaded waves.
- **One block, instanced.** `geometry::build_fan` holds 512 columns of 32 rows of cells, a vertex's position its
  (column, row) and its level −1; `surface.wgsl`'s vertex stage places it from `Surface.fan` and `Surface.detail.zw`.
  The indices run column by column, so a draw takes the columns the view needs and stacks an instance per 32 rows,
  nearest first. The last row stands where the fan ends (`Fan::far`), whatever its place in the lattice: a coarse
  tier's rows are deeper than the sea's horizon dips, and the lattice alone would stop short of it. Water beyond the
  far plane is drawn along its line of sight just inside it, as the horizon ring's is.
- **What keeps the clipmap**: the normal lens (its frames are unchanged), an eye within 10 m of the surface (a
  periscope, a low free camera: the water at the lens is the sea the underwater probe reads), a view whose forward
  lies less than half along the sea (past 60° down) or whose lowest line of sight looks more than 70° down (the water
  lies all round the point under the eye), and one wider than a half turn. Those still centre the clipmap as the next
  section has it.

**Cost.** Apple M5 Pro, 2026-10-12, High, 1728×1030, `dev-client`, the frame's median over 90 redraws of a frozen
moment (`shot.ts --settle 90`), fan and clipmap alternated in one process, three pairs each: at 8× 8.9 ms against
7.3 (+1.5 ms), at 2× 7.9 against 6.6 (+1.3), at 24× 9.6 against 8.4 (+1.2). The same eye at the normal lens: 8.5 to
9.0 ms. A magnified frame had been cheaper than a normal one, its mesh two coarse levels and the ring; it now costs
what the normal lens's does. With square cells to the horizon the fan added 3.6 ms at 8× (158,000 cells against
40,000); a fan drawn flat cost as much as one with its waves, so the cost is the triangles' (their vertices, and the
pixels thin ones shade twice), not the relief's. Rows 4 pixels tall add 0.7 ms and leave relief in the lower two
fifths of the sea.

Tests: `geometry.rs` (the fan covers every line of sight in the frame, turned, tilted and rolled; its lattice holds
under a turn, a tilt, a lens a hair stronger and an eye a metre higher; rows no thinner than the floor; the block
winds as the clipmap does) and `tests/surface.rs` (`a_magnified_view_s_fan_covers_the_sea_in_the_frame`: on High and
Low, water in every pixel from the horizon's row down at 2×, 8× and 24×, none in a view of the sky).

## The clipmap follows a magnified view (2026-10-03)

The clipmap's cells grow with distance from its centre, which matches a pixel's footprint only at the normal 52° lens.
Through binoculars a hull 2 km off fills as many pixels as one 250 m off at 8×, but she rode 16–32 m cells (of the
mesh as it was then: "The water mesh follows the view's standoff" below), and the bow
waves' vertex crest (`wake.wgsl`, `finest`) flattened to suit them: a 2 m crest drew about 0.4 m, and the bow spray,
which rises from the full crest, stood over flat water. `geometry::focus` now moves the centre from the eye (up to
1.25×) to what the view is aimed at (from 2×): the coupled hull nearest the middle of the view (`set_hull_sea`'s
footprints, within 1.2 half-heights of it), or else where the view ray meets the sea, at most 12 km off. The surface's
geomorph and the bow waves' vertex filter measure from that centre (`Surface.clipmap.zw`, `Wake.torpedo.zw`), so the
hull heights and the underwater probe read the sea as drawn. At the normal lens the frame is byte-identical. Since
2026-10-12 the hull heights also read the sea at the mesh's spacing where the hull stands (`wake.wgsl`'s
`mesh_spacing`, `waves.wgsl`'s `drawn_height_at`): a hull past the focus's 12 km, or off the lattice on the horizon
ring, stands in water drawn flat, and with the wave field's full heights in her patch the plating between the two was
lit as under water, black strips along the waterline of a ship 9 nm off through binoculars. The water
between the eye and the focus, at the bottom of a narrow view, takes coarser cells than before; its normals still
carry the short waves. A hull change under the sight moves the centre at once (no easing). Since 2026-10-12 a magnified view from 10 m or
more above the surface draws the fan above instead, and this centring is left to the views that keep the clipmap. An eye within 3–10 m of the highest
the surface reaches (a periscope, a low free camera) keeps the centre, so the water beside it is the sea the underwater
probe reads.

## The open sea's swell is short-crested (2026-10-05)

With combat's sea drawn everywhere (`HullSeaUniforms::set_everywhere`), its two long sines crossed the whole sea as one
even corrugation, ruled straight to the horizon. The first fix (#1007, 2026-10-04) gave each sine groups (a value noise
on its height, 3 wavelengths along its run and 7 along its crests) and wandering crests (a noise on its phase, turning
them up to about 20°), but from a few hundred metres up the owner still read the open sea as geometric and tiled: the
crests still ran parallel for several wavelengths, and the noise's cells, laid along and across them, showed as blocks.

Now away from the hulls each sine is one of a spread of components, as a real sea is a spread of waves and not one
(`hull_sea::open_sea_swell`, which `shaders/waves.wgsl` reads as `HullSea.swell`). The first sine (0.84 of the energy)
has 12 more, the second 4. Their directions are spread evenly within ±0.8 rad (46°) of their sine's, their wavelengths
0.70–1.42 times its own on a golden-ratio sequence, their phases scattered; each runs at its own speed on the
simulation's clock and holds an equal share of its sine's energy. Their interference gives crests a wavelength or two
long and sets that build and fade, with nothing ruled or repeating. Near the hulls (`swell_freedom`, unchanged) the
spread hands its energy back: a sine's amplitude is √(1 − f²(1 − share)) and the spread's f, so the variance holds at
every distance and each hull rides combat's sea exactly (`swell_height` on the CPU, its tests in `hull_sea.rs`;
`tests/hull_heights.rs` on the GPU).

- **How it was found and judged**: stills from 150 to 1,200 m up at 9 m/s, clear, noon (`bun native/tools/shot.ts
  --battle "bismarck;;fletcher:static" --range 20000 --weather clear --wind 9 --time noon --views <views.json>`), each
  image row's mean removed and the sea's contrast stretched. With the open sea's swell set to zero the lines vanished,
  which pinned them on the swell. A spread of ±29° and 0.78–1.28 times was tried first: shorter crests, but still lines
  across the view from 900 m up; ±46° reads as a natural sea at every height.
- **Its highest waves**: eighteen components can stack into a set higher than two sines' 0.93 Hs, as a random sea's
  do; the hulls still ride the two sines.
- **Cost**: 16 more sines per vertex and per pixel of the water in place of the noise's four reads; not measured
  separately.

The hex tiling of the 181 m and 31 m cascades (PR #1004) was reverted on 2026-10-04: the lattice it removed was not
what the owner saw, and it cost 0.2–0.6 ms.

## FFT patch repetition in sun glitter (2026-10-06)

With gusts disabled, overhead views into the sun exposed repeated wave patches. Scattering only the middle
cascade's 181 m tile reduced one repeat but left visible repetition in the after picture. The initial normal-field
regression passed because it deliberately retained the longest cascade's periodic sampling; it did not establish
that the finished surface was free of repetition.

The follow-up isolated the remaining contribution in a frozen native scene (9 m/s, 16:30, seed 1941). Suppressing
the longest cascade's slopes removed its 1,024 m image correlation; suppressing fine waves, ripples, mesh displacement
or the first cascade's filtered variance did not. Scattering the longest cascade reduced that correlation too,
without removing its wave detail (`.build/native/ocean-tiling/detail-*.png`).

`waves.wgsl` scatters both coarser cascades: a rotated triangular grid chooses three independently shifted copies,
with cubed barycentric weights normalized so their squares sum to one. Displacement, slopes and strain keep their
variance; foam, bubbles and each copy's filtered slope variance mix by the squared weights, keeping their means.
The surface uses explicit texture gradients from the pixel's world footprint, so a triangle boundary's random UV
shift cannot select a coarse mip and draw a seam. Vertex displacement, inverted sea heights and spindrift use the
same copies. The first cascade's hull-sea lowpass uses those copies too: subtracting the original periodic field
would otherwise leave a mismatched wave beneath the hulls. The finest cascade and ripple reuse retain their original
sampling. Low scatters its one cascade; Medium, High and Ultra scatter their two coarser cascades. This adds reads to
the longest cascade; its cost has not been measured separately.

The GPU regression (`tests/tiling.rs`) now renders the complete sunlit surface, including its horizontal Jacobian
and filtered roughness, with gusts disabled. In a fixed overhead view, the previous fix's 1,024 m highlight correlation
was 0.208; the two-cascade sampler gives 0.008 and 0.013 along the two axes. The 181 m correlations are 0.015 on both
axes. The test requires absolute correlation below 0.15 and nonzero wave contrast. The field-level check uses jittered
samples over eight tiles, avoiding the old regular lattice's biased energy reference: slope-energy ratios are 0.978
and 1.003, within the required 15% of the original. Matching native captures at 70, 700 and 1,000 m use the frozen
reproduction, with gust strength zero (`.build/native/ocean-tiling/corrected-*.png`).

## The water mesh follows the view's standoff (2026-10-04)

The web's clipmap, ported as it was, had six levels from a 256 m square whatever the view: 4 m cells on High (16, 8 and
2 m on the other tiers), each level's cells 2/64 to 4/64 of their distance from the centre, 40 to 70 pixels across at
1080p. Beside a hull from a low camera the water was 4 m facets, and every wave shorter than about 16 m was in the
normals alone. Now (`geometry.rs` `Clipmap`, `QualityTier::clipmap`):

| Tier | Cells a level edge | Levels | Finest cell | Was |
| --- | --- | --- | --- | --- |
| Low | 32 | 6 | 8 m | 16 m |
| Medium | 64 | 7 | 2 m | 8 m |
| High | 128 | 9 | 0.25 m | 4 m |
| Ultra | 256 | 9 | 0.125 m | 2 m |

- **Every tier's lattice reaches 4,096 m** from its centre (`CLIPMAP_REACH`, as before), where the horizon ring takes
  over; a level is half the one around it, so more levels are finer levels in the middle. A level's cells double the
  cells a level edge holds, so a cell covers half the angle it did at every distance (20 to 37 pixels at 1080p on
  High).
- **The finest level drawn is the first as wide as the view's standoff** (`Clipmap::detail`, `geometry::standoff`): the
  eye's distance from the water at the clipmap's centre, less the significant wave height, over the lens's
  magnification. A level narrower than that holds cells smaller than the levels beyond it show on screen, so a
  periscope or a lens at the waterline draws all nine levels (25 cm cells within 16 m), a bridge 25 m up and the chase
  camera start at the level 32 m wide (50 cm; the chase camera's own hull, 150 to 250 m off, rides 2 to 4 m cells
  where she rode 4 to 8), and a view from 1,000 m up starts at the level 1,024 m wide (16 m).
  The mesh holds every level's middle and its ring apart (`ClipmapMesh::fills`, `rings`): a frame draws the first
  level's middle, then the rings from it out and the horizon, two draws. Under water the same holds by depth.
- **The hand-over never shows**: over the second half of the way to the next level the finest level drawn morphs as a
  whole onto the next one's lattice (`Surface.detail`, `receding` in `surface.wgsl`'s vertex stage, the geomorph the
  levels' outer edges already use), so by the standoff that drops it, it is the middle of the level that takes over,
  vertex for vertex.
- **Binoculars** bring far water as near as its distance over the magnification: 8× on a hull 2 km off is a standoff of
  250 m, the level 256 m wide, 4 m cells about her on High where she had 4 m before on the coarser lattice (8 m a
  level out), 2 m on Ultra.
- **The bow waves' vertex crest** flattens by the mesh as drawn: `Wake.bow.w` is the cell of the finest level drawn
  (doubling as it morphs, so it does not step when a level is dropped), set each frame with the mesh's centre, and
  `Wake.field.w` how fast the cells grow with distance from the centre (two over the cells a level edge holds); the
  hulls' wet band, the underwater probe and the rain's splashes read the same crest (`wake_height`).

Triangles on High: about 230,000 from a periscope, 180,000 from the chase camera, 107,000 from 420 m up, against
39,680 always. `shaders/surface.wgsl` takes the frames of up to ten levels (`MAX_CLIPMAP_LEVELS`).

**Cost** (Apple M5 Pro, Metal, 2560 × 1440, High at its 70 % scale, `water-cost.ts` back to back on master's shaders
and these in one worktree, 120 frames a view, load average 9 to 19 with other sessions' builds and clients on the GPU):
the water and sky pass

| View | Before | After |
| --- | --- | --- |
| chase | 2.97 ms | 3.32 |
| bridge | 2.33 | 2.55 |
| low | 2.08 | 2.40 |
| close | 2.86 | 3.14 |
| overhead | 3.29 | 3.44 |

The same mesh with every level collapsed onto the next (its vertices, half its triangles on screen; the morph forced
to 1 through hot reload) was 0.1 to 0.2 ms cheaper in the bridge, low and overhead views and the same in the chase
view (two rounds each at load 40 to 50): about half the added cost is the vertices and half the smaller triangles'
shading. Ultra's 256 cells a level were measured only at load 25 to 50, where runs differed by a factor of two; the
low views came out 0.5 to 1 ms dearer of 5 to 9. Measure it on a quiet machine before changing its count.

**Left open.** The rings are drawn whole: most of their vertices are behind the eye or beside the view. Drawing each
ring in blocks culled against the view would take back the vertices' share (0.1 to 0.2 ms on High, more on Ultra). The
far levels' cells are slivers a few pixels tall at grazing views; collapsing levels beyond the range where a wave's
height is a pixel would take back some of the shading's.

Stills, before and after, plain and with the mesh's cells drawn over the sea: `.build/native/water/`
(`stills.sh <label>`; `wire.ts on|off` patches the cells' edges into `surface.wgsl` through hot reload, coloured by
level, and takes them off again).

## The sea's GPU cost on High (2026-10-03)

Measured with `bun native/tools/water-cost.ts` (a duel held still, five views, the ocean on and off) and an A/B of
shader variants swapped in one client through hot reload, on Windows (RTX 5070 Ti, Vulkan, 2560 × 1440, High). Another
client shared the GPU throughout, and its time slices land inside our passes and inflate the medians, so the numbers here
are each view's fastest frames (min/p10), which held steady from run to run.

- **Where it went**: the water pass took 1.0–1.8 ms. A flat-coloured sea costs 0.12 ms, so nearly all of it is the
  fragment shader. Screen-space reflections were about half of that: 0.37 ms on the bridge view with no ship in sight,
  0.9 ms overhead. Every other part costs 0.02–0.12 ms on its own (field reads, sky lobe, foam, wake, groups, gusts,
  fog). The shader is register-bound: removing code that never runs still saves about 0.1 ms.
- **Reflections trace only rays that can meet something** (`src/reflection_tiles.rs`, `shaders/reflection_tiles.wgsl`,
  `SceneLayer::after_copies`): after the opaque copies a compute pass marks the depth copy's 16-pixel tiles holding any
  surface and sums them into a table in a storage buffer (the fragment stage already binds WebGPU's 16 sampled
  textures). A ray whose clipped screen box holds no surface returns at once with what its march would have found
  (nothing). Only surfaces a ray can reach are marked (`reach`, since 2026-10-09): nearer than the drawn sea's edge (a
  20 m crest's distance past its horizon, where the surface closes its rows up) and the longest ray beyond, and never
  past a battle view's far plane (`REACH_M`, 60 km); a uniform holds that depth for the frame's camera, 0 when the far
  plane is no farther. A theatre view drawing the land beyond its window (`terrain.md`, "The horizon ring") had that
  land's band along the horizon in every ray's box from the bridge, and each was marched for nothing (the water pass
  +0.4-0.9 ms from a ship's bridge). Rays whose camera-facing trust is 0 are not traced, the march reads its depths
  four at a time, and the bisection reads two halvings' points at once. The picture is bit for bit the same in every
  view, and `check.ts` gives the same numbers as master on every check; only from high up is the ring's land past 60 km
  left unmirrored, as a battle view drew nothing there.
- **Sky reads share their work** (`sky_environment.wgsl`, `reflection.wgsl`): one face lookup per direction for both
  blended levels, one roughness level for the lobe's three taps. The same up to rounding (a few pixels per million move
  a few levels).

| View | Water pass before | After |
| --- | --- | --- |
| chase | 1.57 ms | 1.29 ms |
| bridge | 1.00 | 0.75 |
| close | 1.50 | 1.23 |
| overhead | 1.83 | 1.15 |
| low | 0.94 | 0.70 |

Tried and left out: passing over each empty four-step batch inside a traced ray (about 1 %). Still open: reflections
cost about 0.55 ms in the chase and close views, for rays crossing the hull's tiles in front of it (a nearest-depth bound
per tile could pass over those), and the register pressure (splitting the shading into passes).

**On Apple's GPU (M5 Pro, Metal, 2592 × 1545, High at full scale, 2026-10-03, load 2–4).** The sea costs far more here
than on the RTX: 3.8–6.1 ms of a 9–13 ms frame by `water-cost.ts` (chase 6.07, bridge 4.21, low 3.75, close 5.14,
overhead 6.00), the water and sky pass 4.4–6.2 ms with the sea and 0.8–1.3 without. Shader variants swapped in one
client through hot reload (the fragment cut short at each stage, the compiler dropping everything after; overhead,
chase and bridge views, ms of that pass):

| Stage | Overhead | Chase | Bridge |
| --- | --- | --- | --- |
| A flat colour (raster, vertex, the sky's share of the pass) | 1.40 | 1.50 | 1.53 |
| + the wake | 0.19 | 0.42 | 0.40 |
| + the waves (three cascades) | 1.13 | 0.99 | 0.74 |
| + shadows, cloud shadow, rain, the lobe, the shelter | 0.47 | 0.28 | 0.30 |
| + the water body | 0.13 | 0.00 | 0.33 |
| + the sky lobe | 1.08 | 0.58 | 0.48 |
| + screen-space reflections | 1.40 | 1.74 | 1.18 |
| + glint and crests | 0.21 | 0.10 | 0.11 |
| + foam | 1.09 | 0.72 | 0.67 |

- **Reflections cost about 1 ms by being compiled in**: turned off at run time they still cost 0.9–1.4 ms of their
  1.1–2.0 (compiled out). Not the batched march (one read a step and one halving a round: within ±0.1), not where the
  trace sits in the shader (traced last, after the foam: +0.05–0.23), not unrolling (run-time loop bounds: ±0.15), not
  the tile table's storage reads (left out: no change). Most likely the register allocation of the whole shader; there is
  no Xcode here to read its pipeline statistics.
- **Not the cost**: anisotropic field reads (isotropic at the same level: 0 to −0.28), the bubble cloud's stretched read
  (~0). The sky lobe's three taps against one: 0.3 ms.
- **No look-neutral win found**; the sea's cost now falls with the render scale, which every preset below Ultra
  lowers (temporal upscaling, `aa-tracers.md`): at High's 70 % the water and sky pass is 3.9 ms (reflections about 0.7 of
  it, 0.37 of that by presence). The A/B: `bun native/tools/shader-ab.ts <variants.ts>` (variants as find-and-replace
  edits, swapped through hot reload in one client, each view's pass timed).

## Weather at sea (2026-10-03)

The web's sea took no rain and threw no spray of its own. Native only since, all from the weather the client already
has (the sky scene's precipitation, the battle's wind): a dry sea under 13 m/s draws the same bytes as before.

- **Rain on the water** (`shaders/rain.wgsl`; `OceanSurface::rain`, 0 dry … 1 a downpour, which naval-client sets each
  frame from the sky scene's precipitation, the console's included): every drop sends a ring of capillary waves across
  the water. Where a pixel resolves them the rings go into the surface's normal (two sizes of drop on a jittered grid,
  each landing somewhere new every life, the front running out at 0.24 m/s); where it does not they are slope variance
  (0.02 in a downpour, 60 % of it drawn as rings where they resolve), so the mirrored sky blurs and the glitter spreads
  and dims.
- **Spindrift** (`src/spindrift.rs`, `shaders/spindrift.wgsl`): from 13 m/s (Beaufort 7; everywhere by 17.5) the ocean
  draws wisps of spray torn off its own breaking crests, where the whitecaps' fresh foam shows through the same breaking
  groups and crest gates the surface draws them with (`breaking_group` moved into `naval_ocean::waves`). No particle
  lives on the CPU: each wisp is a slot in four levels of 48² cells laid on the world about the camera (2.5 m at the
  finest, each level twice the last over its own band of distance, three wisps a cell), driven downwind, growing and
  thinning over its life. They draw in `Phase::Transparent`, lit as the splashes' white water (`naval-water-effects`'
  `spray_radiance`, whose constants `spindrift.wgsl` copies: that crate depends on this one), with lightning's flash the
  host sets in `ocean.spindrift.flash`, and fade into the sea and hulls behind them through the backdrop's depth
  (`SceneCopies::BACKDROP_DEPTH`, asked for only while the wind makes spindrift). Since 2026-10-04 a wisp's beam takes
  the ships' (and splash columns') shadows as the sea beneath it does, besides the clouds': `water_sun_shadow` at its
  centre in the vertex stage, at the surface's water-shadow quality (`SprayUniforms::spray.w`, which `Ocean::prepare`
  passes from `surface.water_shadows`) and faded under the moon as the surface fades it. Spray torn off a crest in a
  hull's shadow no longer glows white in it. One read per live wisp, in the vertex stage: no cost the noise could show.
- **Spray haze** (`spindrift::spray_haze`, `shaders/spray_haze.wgsl`): from 17 m/s a haze of spray lies over the sea,
  its extinction 1.8e-4 /m at 20 m/s doubling every 3 m/s (at most 0.01 /m) and falling off above the sea over a scale
  height of 3 m plus 0.4 of the significant wave height, integrated exactly along each view ray: visibility in it about
  14 km at 22 m/s and 3.4 km at 28 (Beaufort 9 to 12, "spray may affect visibility" to "very seriously"). The surface
  veils the water with it before its distance fog, and the wisps take it too (`Surface.weather` yzw).
- **Gale foam streaks** (`field.rs` `streak_params`, `shaders/resolve.wgsl`, `foam.wgsl` `gale_streaks`): from 13 m/s
  (full by 20) the second cascade's displacement `w` holds old foam (181 m on High, texels of 0.7 m): fed by its
  whitecaps, carried each update by the surface's drift (3.5 % of the wind: the wind-driven current and the waves'
  Stokes drift) and gathered by two meandering sets of Langmuir convergences 18 m and 29 m apart, with the continuity
  equation's compression (passive advection never gathers), decaying over 30 s. The surface draws it as the wind's
  streaked lace in long patches laid on the world. Top-down at 22 m/s the sea goes from 5.9 % to 10.5 % white, and its
  foam marks from 15 to 58 m long (aspect 1.9 to 6.8).
- **Rain's splashes on the drawn sea** (`src/rain_heights.rs`, `shaders/rain_heights.wgsl`): naval-clouds' rings and
  crowns stood on the flat mean sea, floating over troughs and hanging in front of a hull's boot-topping. While it rains
  the ocean evaluates its own surface (`height_at` and the wakes' `wake_height`, as the hulls' wet band does) on a 256 m
  square about the camera at a metre a texel, laid on the world so it repeats; `Ocean::rain_heights` hands it to the
  clouds' weather (`Weather::set_sea_heights`, which naval-client's `scene.rs` wires; `clouds.md`).

**Cost** (Apple M5 Pro, 2400 × 1350, a Fletcher in a 22 m/s storm and heavy rain, the camera 140 m off at 7°, load
average 7–15; six alternating rounds of 120 frames, the client's own shaders against a variant drawing none of the above
swapped in through hot reload; the fastest frames): the whole frame 9.61 ms against 9.58, the transparent pass 0.93
against 0.99, water and sky 4.53 against 4.54, all within the noise. The rain's heights take 0.068 ms of compute, only
while it rains; the streaks' update rides in the wave field's pass (0.36 ms either way).

**Left open: an opaque veil.** The spray haze veils only the water and the wisps; hulls, land and aircraft standing in
it are not veiled. A pass over the opaque scene (or the haze in the fog every material shares) was left out.

## Run it

```sh
cd native
cargo test -p naval-ocean                      # CPU parity with the TypeScript, GPU FFT vs CPU DFT, every tier draws
cargo run -p naval-ocean --example stills --release [-- --only bridge9,glint9] [--frames 120]
# web half (from the repo root, after `bun run bootstrap`); writes .build/native/ocean/web/
bun native/crates/naval-ocean/web/ocean-stills.ts [--only bridge9] [--winds 9,15,25]
```

The example reads every scene the web driver wrote (camera, wind, sun, the web sky's equirectangular
bake), renders the same frame natively through `naval_render::Renderer` (the ocean and a stand-in sky
as its layers, its display pass for the game's grade and AgX) and writes:

- `.build/native/ocean/native/<scene>.png`: the native frame (1600 × 900);
- `.build/native/ocean/side-by-side/<scene>.png`: web left, native right;
- `.build/native/ocean/diff/<scene>.png`: native − web, red brighter, blue darker (×4);
- `.build/native/ocean/compare.json` and `timings.json`.

Without web data it draws the same poses under a gradient sky.

Scenes (north-atlantic map, clear sky, cameras ≥ 2.5 km from the ships the page places, fov 55°):
`bridge` (0, 30, 2500) → (0, −22, 4500), `chase` (0, 60, 2500) → (0, −240, 4600), `overhead`
(0, 1200, 2000) → (0, 0, 3400), all at noon with the sun behind; `glint` at 16:30, 30 m up, 2.5 km
toward the sun and facing it. Each at 9 m/s (the port's sea), 15 and 25: `bridge9` … `glint25`. The
waves are frozen alike on both sides: foam wiped by one update at t = 50 − 1/30 with dt = 1000 s, 300
updates at 1/30 s to t = 59.97, then a paused frame at t = 60. The web's eye adaptation is locked off
(display exposure 1), so neither side meters.

## What is ported, file by file

| Native | Ports | Notes |
| --- | --- | --- |
| `src/spectrum.rs` | `waves/spectrum.ts` | f64 step for step, amplitudes stored f32, PCG on u32: same seed, same sea. Tests pin `modeHash`, per-cascade variance/slope variance/slope length and texels to Bun's output. |
| `src/sea_state.rs` | `waves/seaState.ts` | Realistic sea state (on, as in the game): 70.4 m peak at 9 m/s, γ, breaking choppiness, grown tiles. |
| `src/whitecaps.rs`, `src/groups.rs`, `src/gusts.rs` | `waves/whitecaps.ts`, `groups.ts`, `gusts.ts` (CPU halves) | Breaking weights, whitecap area/depth, group cut and layout, gust strength: pinned to Bun. The breaking window has moved since (above), so the thresholds, shares and periods are the native crate's own. |
| `src/calibration.rs` | `maps/seaCalibration.ts`, `VisualEnvironment.applySea/applyFog` | The wind-sea table and North Atlantic water copied (tests check them against the JSON); port and battle parameters. |
| `src/foam_texture.rs` | `surface/foamTexture.ts` | 256² RGBA8 with its equalised mip chain; texels match Bun's byte for byte. |
| `src/field.rs`, `shaders/fft.wgsl`, `resolve.wgsl`, `mips.wgsl` | `waves/field.ts` (`evolve`, `pass`, `resolve`, `spreadFoam`, `update`, `setBreaking`), `waves/fft.ts` | Same Stockham radices (16·16 on 256², 8·8·8 on 512²), as compute on f32 storage buffers; fields in `Rgba16Float` 2D arrays with box mips to 16². A test checks the GPU displacement against a CPU inverse DFT (≤ 2e-3 relative) and the mip average. |
| `src/geometry.rs` + vertex stage of `shaders/surface.wgsl` | `surface/OceanGeometry.ts`, `field.displacement` | The clipmap (the web's six levels from a 256 m base; since 2026-10-04 more cells, finer levels and the finest drawn by the view's standoff, above), geomorph, horizon ring to 95% of far in circles 3.5 km apart (15 km apart from 60 to 480 km, where a theatre view draws the land beyond its window: `terrain.md`, "The horizon ring"), and a skirt on to the sea's horizon from an eye that sees past the ring (the Earth's curve is applied per vertex: `render-core.md`, "The Earth's curvature"). |
| fragment stage of `shaders/surface.wgsl` | `field.surface` (+ ripples), `gusts.gustGain`, `groups.breakingGroup`, `OceanSurfaceMaterial.bind`, `screen/fog.ts` | The physical graph the game draws (`realism.reflections` and `realism.waterColor` on). |
| `shaders/reflection.wgsl` | `surface/physicalReflection.ts` (+ `smithLambda`, `softKnee`) | Reflection lobe, mean Fresnel, three-tap sky lobe, Beckmann glitter. |
| `shaders/water_body.wgsl` | `surface/waterBody.ts` | Lee et al. water body, column transmittance. |
| `shaders/foam.wgsl` | `surface/foam.ts` | Whitecaps (core, residue, spread, far mean), windrows, billows, bubble cloud, foam light; the gale streaks (native only). |
| `src/spindrift.rs`, `src/rain_heights.rs`, `shaders/spindrift.wgsl`, `spray_haze.wgsl`, `rain.wgsl`, `rain_heights.wgsl` | (none: native only) | Spindrift, the spray haze, rain on the water and the sea's heights for the rain's splashes ("Weather at sea"). |
| `src/caustics.rs`, `shaders/caustics.wgsl` | (none: native only) | The sunlight the waves gather on what stands under the sea, a layer per depth about the camera; the hulls read it on their submerged plating (`sea-sky.md`, "A view to work from, for the U-boats"). |
| `shaders/noise.wgsl` | `noise.ts` | Integer-hash value noise. |
| `src/ocean.rs` | `Ocean.ts` (the part that is ported) | `Ocean { parameters, time, field, surface }`, a `SceneLayer` (see "Renderer core"). |
| `src/environment.rs`, `shaders/sky_environment.wgsl`, `shaders/environment_fill.wgsl` | the sky lane's interface; stand-in for its prefilter | See "Environment". |
| `src/backdrop.rs`, `shaders/backdrop.wgsl` | a sky behind the sea | `StandInSky`, a Sky-phase layer drawing the environment atlas; for stills only. |
| `src/lakes.rs`, `shaders/lake.wgsl` | (none: native only) | Lakes and ponds at their own levels, a `Water`-phase layer of their own ("Lakes and ponds"). |

One deliberate difference from the brief: the spectrum is **initialised on the CPU**, as the web does,
not in WGSL compute. The whitecap thresholds, the gates, the tail roughness and the height bounds all
need the realised spectrum's statistics on the CPU, and building it there keeps the same seed drawing
the same sea as the web bit for bit (the tests pin it). It runs once per sea change (a few
milliseconds in release). Evolution, the transform, foam and mips are GPU compute every frame.

## How it matches

Mean sRGB colour of the sea (below the horizon) on each side, mean absolute difference per channel,
and the share of pixels more than 24 levels apart (`compare.json`):

| Scene | Web mean | Native mean | Mean abs. diff | > 24 levels |
| --- | --- | --- | --- | --- |
| `bridge9` | 35.6, 75.4, 106.5 | 36.0, 75.7, 106.8 | 3.59 | 0.8% |
| `chase9` | 29.5, 67.5, 97.2 | 29.9, 67.9, 97.6 | 3.13 | 0.6% |
| `overhead9` | 10.5, 35.4, 55.7 | 10.5, 35.5, 55.8 | 1.27 | 0.5% |
| `glint9` | 113.0, 122.5, 133.1 | 111.4, 121.2, 132.1 | 5.21 | 3.7% |
| `bridge15` | 31.6, 70.3, 100.2 | 32.0, 70.8, 100.7 | 3.42 | 0.9% |
| `chase15` | 28.9, 65.3, 93.5 | 29.4, 65.9, 94.0 | 3.19 | 1.2% |
| `overhead15` | 21.0, 44.9, 63.6 | 21.4, 45.4, 64.0 | 2.61 | 2.2% |
| `glint15` | 114.3, 122.2, 130.8 | 114.2, 122.2, 130.8 | 5.05 | 4.2% |
| `bridge25` | 42.5, 75.0, 100.3 | 43.0, 75.6, 100.9 | 3.86 | 3.2% |
| `chase25` | 46.6, 75.7, 98.7 | 47.2, 76.3, 99.3 | 3.97 | 3.8% |
| `overhead25` | 65.8, 81.5, 95.3 | 66.1, 81.9, 95.7 | 5.01 | 5.2% |
| `glint25` | 122.9, 128.0, 133.8 | 123.1, 128.2, 134.0 | 4.51 | 3.9% |

The waves, every whitecap and windrow, the glitter path, the far-sea gradient and the horizon land on
the same pixels (`side-by-side/*.png`). The difference images show no structure in the sea, only
pixel-scale noise on crests, foam edges and glints, which is what the web's FXAA and bloom (threshold
2) and the two GPUs' anisotropic filters leave; the native sea is 0.3–0.5 sRGB levels brighter on
average. The parameters the native side derives from the presets equal what the page reported for
every scene (the example prints any that differ; none do).

What does not match:

- **The sky above the horizon** is a stand-in: the environment atlas's sharpest level drawn as a
  backdrop, filled from the web's 384 × 192 bake, so clouds are blurred and there is no sun disc. The
  sky lane draws the real dome.
- **Environment prefilter**: the stand-in fills each atlas level with one Gaussian at three's total
  sigma; three reaches it through a chain of separable blurs. The sea's colour agrees within a level,
  so the difference is small, but the sky lane's port of the chain replaces it.
- ~~**Fog colour behind the fragment**~~: done by the sea-sky lane (`sea-sky.md`), then replaced on 2026-10-03: the
  surface takes the aerial perspective (`apply_fog` reads `naval::aerial`, `haze.md`), and the fog map is gone.
- **No bloom, FXAA or eye adaptation** in the stills (the renderer core's display pass has the grade
  and AgX only; the web's eye was locked for the comparison, the game would open the overhead views up
  by up to two stops).

## Renderer core (`naval_render::SceneLayer`)

**Render core II (integration lane, batch 3):** the surface draws in `Phase::Water`; with
`Ocean::read_scene_copies` it asks for `OPAQUE_COLOR | OPAQUE_DEPTH` and binds `scene.opaque.color`/`depth` when
their version changes; `lit` and the glitter's beam take the web's `shadow` node, `water_sun_shadow(position,
OceanSurface::water_shadows)` (off by default; every sun map, the view maps since 2026-10-05) times the clouds' shadow (`Ocean::set_cloud_shadow`, group 1
bindings 11–13, a disabled placeholder until set); `draw_motion` writes `motion_response(1.0)` with a second,
position-only draw of the clipmap. Earlier text below that says "opaque phase" predates this.

`Ocean` implements `SceneLayer` (`native/docs/render-core.md`): `prepare` advances `ocean.time` by the
frame's `dt` (a paused frame, `dt` 0, steps nothing), encodes the wave field's compute pass on the
frame's encoder, encodes the wakes' GPU half when `Ocean::enable_wake` made them (`Ocean::prepare_wake`),
and writes the surface's clipmap snap from `ctx.camera.position`; `draw` draws the
surface in `Phase::Opaque` (depth written, `scene_depth_state(true)`, `HDR_FORMAT`). The surface
shader imports `naval::camera::camera` (registered in this crate's composer from
`naval_render::shaders::builtin::CAMERA`) at group 0, and its own groups are 1 (surface uniforms, foam
texture, scene copies, and the wakes at bindings 5–10), 2 (wave field) and 3 (sky environment). Build it with the renderer's camera
layout: `Ocean::new(device, queue, tier, &ctx.camera_layout, &env.bind_group_layout, &env.bind_group,
far)`. `StandInSky` is a `Phase::Sky` layer for the stills.

Done by the pipelines lane (`Ocean::new_in`, `enable_wake_in`; `native/docs/render-core.md`, "Pipelines: no
first-launch stall"): the field's, surface's, wake field's and stamp painters' pipelines are registered on the
renderer's registry from the library's copies of the ocean's files (`shaders::FILES`), compile on its build threads
and hot-reload; `Ocean::new` still builds its own for standalone use. Before it: the pipelines were built with the
device, not through `RenderContext::add_scene_pipeline` /
`add_pipeline`, so they do not hot-reload. Moving them means registering this crate's WGSL files with
the renderer's `ShaderLibrary` (`shader_file!`) instead of its own composer, and the compute pipelines
as recipes; the bind group layouts stay the crate's.

**What the ocean needs from the renderer for the slice: copies of the opaque scene.** The web draws the
sea first in the transparent queue so three's viewport copies hold the opaque scene; the surface reads
them for straight-through transmission (water body over submerged hulls, shallows) and shoreline foam
(where the column behind the water thins: a hull's waterline, a beach), and screen-space reflections
will read them too. Wanted: after every other layer's opaque draws and before the ocean draws, copy
`scene.hdr` (`Rgba16Float`, full size, sampled with `textureLoad`, non-filterable is enough) and
`scene.depth` (`Depth32Float` as `texture_depth_2d`), then draw the ocean with depth test and write
on, then the Sky phase. In phase terms: Opaque → copy → a water step (or the ocean first in
Transparent with depth write, as the web does) → Sky → Transparent. The sky is not in the copy (the
web's copy is black where sky will be, and the reflections already account for that). Hand the copies
to the ocean once with `Ocean::set_scene(device, color, depth)` (which keeps the wakes bound; the
surface's own `set_scene` binds a calm wake) and again when the viewport resizes (the resources'
`version`). Render core II has published exactly this (`SceneCopies::OPAQUE_COLOR | OPAQUE_DEPTH` and
`Phase::Water`); until it merges, the water-effects stills draw their proxy hulls into targets of their own
and hand those over. Until then the surface binds a black, far-plane placeholder, which is the open
sea's result: no hull, no shoreline.

## Environment (the sky lane's interface)

The surface binds the sky lane's published environment (`native/docs/sky.md` on
`goldflag/native-sky`, "Environment lighting") at group 3: binding 0 the `Rgba16Float` atlas in three's
CubeUV layout, 1 its sampler, 2 the 80-byte `SkyEnvironment` uniform. The shaders call
`naval_sky::environment::{env_radiance, env_irradiance}` exactly as the web calls `pmremTexture`:
the reflection lobe converts its angular spread to a PMREM roughness with the web's measured table
(`pmrem_roughness`), the water body and foam read `env_irradiance(up)` and `env_radiance(up, 1)`. The
sun (or moon) the sea shades with is the uniform's `light_direction`, `light_color × light_intensity`:
the web's `ocean.sun` is `sky.light`.

naval-sky is not merged, so this crate carries a byte-for-byte mirror of its `environment.wgsl`
(`shaders/sky_environment.wgsl`, registered under the same import path) and of its Rust types and bind
group layout (`src/environment.rs`), plus `StandInEnvironment`, which fills an atlas of that layout
from an equirectangular map. **The vertical slice swaps in the real module**: register
`naval_sky::add_environment_module` instead of the mirror, delete `sky_environment.wgsl` and the
mirrored types, and pass `naval_sky::EnvironmentMap::bind_group` / `bind_group_layout` where the
stand-in's go. Nothing else changes.

The stand-in atlas is filled from the web's **equirectangular bake** (read back by `web/skyBake.ts`,
row 0 the nadir, checked on the data), through the inverse of the native lookup, so it needs no
flipping. The web's PMREM atlas itself is stored mirrored relative to native lookups (three samples
render-target atlases with y negated: every face upside down, ±Y swapped); anyone comparing against a
read-back of that atlas should convert it as the ships lane's `web_atlas_to_native`
(`native/crates/naval-ships/src/environment.rs`) does.

The fog sampler the web reads (`OceanSky.createFogSampler`) is the environment's binding 3 (`env_fog`, the
sea-sky lane). Note the PMREM blur table the ocean uses was measured on the web's 384 px bake (faces of 64); the review page's High preset uses
that bake, while the sky lane's doc lists 512 (faces of 128) for High, whose sharp levels are finer.

## Parameters from the web presets

`calibration::scene_parameters(map, wind, port)` does what `VisualEnvironment.applySea` and `applyFog`
do:

- **Waves**: `windSea(map, wind)`: significant height and peak wavelength interpolated from
  `seaCalibration.samples` (× the map's amplitude and wavelength scales), choppiness
  0.65 + 0.9·clamp((U − 3)/12); γ 2.6, directional sharpness 0.8, seed 1941 (`Game.ts`); wind direction
  the map's (the port's 35°). With the realistic sea state (on) the field redraws it as a real wind sea:
  peak 70 m at 9 m/s, 146 m at 15, 298 m at 25, Donelan's γ, choppiness 2.25.
- **Wind by weather preset** (`battle-conditions.v1.json`): map 9, clear 5, partly cloudy 9, overcast
  12, fog 5, storm clouds 16 m/s, × the map's wind scale; the port's standing wind is 9 m/s.
- **Foam**: crest opacity 1, wind stretch 0.5, coverage scale = map foam / 0.45 (1 on the North
  Atlantic), lifetime 1 period; windrows opacity 0.5 over `windrowCoverage(U, scale)`; shoreline
  `#edf9fd` at 0.8.
- **Water**: absorption the map's `absorptionColor` (sRGB hex → linear); the physical water body uses
  only it. Pigment and transmission colours pass through for the crest light and the underside.
- **Fog**: port `#819aa5`, 650–5,600 m, power 0.85, sky blend 2,600 m; battle the map's fog with
  power 1.4 (the ramp; real extinction is off in the game).
- **Time of day and cloud**: reach the sea only through the sky (light and environment).
- **Quality tiers**: `QualityTier` (Low 1 × 256², Medium 2 × 256², High 3 × 256², Ultra 3 × 512²;
  clipmap segments 16/32/64/128 on the web, 32/64/128/256 with finer levels since 2026-10-04, above), as `quality.ts`.
  The stills use High.

## GPU cost

Apple M5 Pro, Metal, 2026-09-30, machine shared with other agents' builds and browsers. High tier,
1600 × 900, medians of 120–240 frames per scene (`timings.json`, `--frames N`):

- **Wave field update** (evolve + 3 transform passes + resolve + 4 mip levels, one compute pass):
  **0.31 ms** GPU (timestamp query) at load average 10–15 (00:03), the same in every scene; 0.55–0.9 ms
  in a later run at load 20 with the GPU contended (00:14).
- **Surface draw** (39,680 triangles, full-screen shading) plus the stand-in sky: in the first run,
  with this crate's own passes, frames without the field step took 1.05–1.48 ms serialised (encode,
  submit, wait) against 0.52–0.69 ms for field-only submissions whose GPU part is 0.31 ms, so the draw
  costs roughly **0.8–1.2 ms**, most at the chase and overhead views where the sea fills the frame.
  Render-pass timestamps read zero on this tiled GPU, so this is a wall-clock difference.
- Whole frames through the renderer core (its scene and display passes included) under the heavier
  load: 2.6–5.2 ms serialised, paused frames 2.2–3.8 ms. The load moved these by ±1 ms between runs;
  measure again on a quiet machine before budgeting from them.
- CPU: rebuilding the spectrum (only when the sea changes) and the foam texture (once) take a few
  milliseconds in release.

## Not ported yet, and what each needs from this crate

- ~~**Sea around hulls**~~ (`waves/hullSea.ts`): done by the sea-sky lane (`sea-sky.md`): `src/hull_sea.rs` (the CPU half,
  the split pinned to Bun's), `shaders/waves.wgsl` (`naval_ocean::waves`: the wave field's group, `displacement`,
  `height_at` and the coupling), `Ocean::set_hull_sea`.
- **Wakes**: done by the water-effects lane (`native/docs/water-effects.md`): the wake field
  (`src/wake/`), trail foam and slick (`src/trail.rs`, `src/stamps.rs`), torpedo tracks
  (`src/torpedo_tracks.rs`), bow waves (`src/bow_waves.rs`), hull contact foam (`src/hull_foam.rs`,
  `src/hull_profile.rs`) behind `ShipWake` (`src/ship_wake.rs`), shaded by `shaders/wake.wgsl`, whose
  bindings join group 1 (5–10). The slick stills each cascade's short waves through
  `WaveUniforms::slick_shares` (`field::short_shares(cascades, 25)`), and the churned water, wake bubbles
  and the wake's normal feed `foam.wgsl` and the surface as `OceanSurfaceMaterial.bind` does. Since 2026-10-07 the wake
  field also takes what strikes the sea (craters, domes, a foundering hull's upwelling: `src/wake/impulses.rs`;
  `water-effects.md`, "The sea answers what strikes it").
- **Hull shelter** (`surface/shelter.ts`, the game's `HullContactFoam.shelter`): done with the wakes
  (`hull_shelter`, `shelter_light` and `shelter_reflect` in `shaders/wake.wgsl`, read with the lobe's
  direction and in-plane spread; the upwelling, foam light and mirrored sky take its substitutions).
- ~~**Screen-space reflections**~~ (`screen/reflections.ts`): done by the lighting lane
  (`shaders/screen_reflection.wgsl`, `OceanSurface::reflections`; `native/docs/lighting.md`).
- ~~**Sun and cloud shadows**~~: done by render core II (`water_sun_shadow` times the clouds' shadow, "Renderer
  core" below), with the view maps since 2026-10-05 ("Distant ships' shadows on the sea" above).
- ~~**Underwater view and the underside**~~: done by the sea-sky lane (`sea-sky.md`): `src/underwater.rs`,
  `shaders/underwater_probe.wgsl`, `underwater.wgsl`, the underside in `surface.wgsl`.
- **Floaters, torpedo overlay**: `heightAt` exists as WGSL (`naval_ocean::waves::height_at`, with the wake's
  `wake_height`); they still need a small GPU readback (`waves/sampler.ts`). The hulls' waterline (their wet band) reads
  it through the ocean's patches about each coupled hull (`src/hull_heights.rs`, `shaders/hull_heights.wgsl`; the
  sea-sky lane's `sea-sky.md`, "The wet band from the drawn sea").

No CPU wave sampling was added: no visual in this step needs it. The underwater switch and floating
objects will (a one-frame-late GPU readback, as the web's `WaveHeightSampler`). Combat never reads
these waves.

## Open problems

- Whether the slice's sky bake is 384 or 512 wide on High changes the far sea's reflection sharpness
  slightly; the blur table would need re-measuring for 512 if the sky lane keeps it.
- `OceanSurface` recompiles nothing at run time: `CASCADES` is a shader def, so changing the tier
  rebuilds the pipeline (the web rebuilds the ocean too).

## Re-sync

Web sources re-read against master `6cd598cf2` (2026-09-30) by the re-sync lane (`docs/archive/native-resync.md`); the next re-sync diffs
from there. Nothing under `src/game/ocean` changed. `ShipWake.ts` gained `layTrails` (theatre: a scene's ships come aboard with
150 s of trail laid astern), left for the theatre-native lane; `FleetWakeFoam.ts` changed a comment only.

## Needs from naval-sim

Nothing: the sea around hulls reads naval-sim's public `SeaState` through the render packet's sea (`SeaWaves`).
