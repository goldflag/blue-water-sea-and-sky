# Water effects lane (`naval-ocean` wakes, `native/crates/naval-water-effects`)

Ports the effects that live on and above the water, driven by simulation state: the dispersive wake field, trail foam
and slick, torpedo bubble tracks, the analytic bow waves, hull contact foam and the shelter hull sides give the water
beside them (all shaded by the ocean surface, so they
live in `naval-ocean`), and the shell splashes and bow spray above the water (`naval-water-effects`). Both crates pass
`cargo check --target wasm32-unknown-unknown`. Outputs go to `.build/native/water-effects/`.

Status (2026-09-30): every system is ported and driven frame for frame from recorded simulation state. Replaying the web
game's own recorded inputs, the native wakes land on the web's pixels (mean absolute difference 2.5–5.7 sRGB levels over
the sea in the wake scenes, the difference images showing only the hull models, funnel smoke and pixel-level foam edges)
and the splashes match jet for jet. Hulls are proxies (no ship models yet), so everything that depends on the hull's own
geometry is fitted to a simplified hull.

## Splash rings that broke white (2026-10-10)

From above a fleet in action on Ultra's ocean, every heavy fall left a ring or blob of dense, pebbly white water 15–40 s
after the column, larger than the foam patch under it and lasting half a minute (the owner's screenshot, a theatre
battle at 1×). It was the wake field's breaking foam: the threshold tuned for the hulls' under-resolved waves (a slope
of 0.015 over 12 m) times the fastest nearby hull's speed share, so any hull under way set it for the whole field. A
strike's ring train held it off for 20 s ("The sea answers what strikes it", "Breaking"), but the rings outlive it:
once forgotten, they broke into annuli wherever they were still steep. On Ultra's 1.5 m cells the rings are the
steepest, and every heavy fall broke; on High it was rarer.

A hull's own breaking lies along her sides. Two fields in lockstep, one breaking at 0.015 for its last step only, give
where it happens: a Bismarck at 15 m/s, straight or turning 0.03 rad/s, on High and Ultra, breaks 98.9–99.9 % within
30 m of the line between her bow and stern generators and all of it within 40 m (1.1 beams), while 114 m from either
generator alone (her middle); a Fletcher at 18.5 m/s breaks next to nothing in the field (under 2 % of her foam, only
while turning). So each generator now carries the stretch where its hull's own waves break, from it to her middle
(`GeneratorOptions::breaks_to`), and how far either side (`break_reach`, `ship_wake::BREAK_REACH` 1.25 beams, fading
out to 1.5 times that), and the step pass takes the breaking threshold only there (`wake_field.wgsl`, `breaks` in the
step uniform). The ring trains stay: they still hold the breaking off where fresh rings pass under a hull.

- A hull's breaking is unchanged: the same 30 s runs' total foam (a Bismarck straight 578.37 on High and 2,448.18 on
  Ultra, turning 532.83 and 2,222.24; a Fletcher 62.64 and 226.90) came out the same to the hundredth, and the web's
  breaking figure in `tests/wake.rs` reads 578.54 (web 578.26).
- `a_salvos_rings_never_break_beside_a_hull` (`tests/wake.rs`): on Ultra's grid a 38 cm round and a 70 kg burster
  ring the sea for a minute while a Bismarck steams by 400 m off. The foam about them only fades (0.19 at 5 s to
  0.0001 at 60 s); with the old field (the stretch's reach made unbounded) it rose from 0.05 at 15 s to 1.18 at 20 s
  and 1.67 at 25 s.
- Seen with `bun native/tools/effects-trial.ts --only splash-underway --stills --graphics '{"ocean":"ultra"}'` (added
  for this: a Bismarck at 15 m/s under another's fire, its first heavy fall after 75 s from 420 m up for 52 s), master's
  build (`--binary`) against this one (`.build/native/effects-trial/splash-rings-before-after.png`): master's
  dense white patches at 18–30 s, and a later salvo's at 44 s, are gone; the falls leave the trail's fading lace and
  pale water. On High master's build shows none in this scene. `parity-wake-astern`,
  `theatre-cruise`, `chase-one-ship` and `fleet-measure30` render within 0.03 levels of master's build (both miss their
  references by the same stale margins on this Windows machine).
- Cost: the step uniform grows from 1,616 to 1,872 bytes (slots of 2,048), and the step's generator loop takes one
  more capsule distance a generator, in a pass bound by its texture reads.

## Spray over the bow and water aboard (2026-10-07)

A bow driven into a heavy sea now throws a sheet of spray up and back over her forecastle, sheds the water it ships off
her deck edges, and wets her deck (the ships lane's "Wet ships", `ships.md`). `bow_spray.rs`: `Aboard`, `aboard`,
`BowSpray::{aboard, aboards, drawn_sea, over_bow, shipping_green_water}`, `Bow::on_deck`, `BowSpray::curtain`, and two
streams in `emit`.

**The sea against the stem.** The hulls ride the combat sea, whose long component is four times the spectrum's peak
wavelength (242 m in a 22 m/s storm, Hs 7.5 m): a destroyer heaves and pitches with it, and on it alone the water at her
stem never came within 3.5 m of her deck edge, a storm or not. What meets her bow on screen is the drawn sea: the short
waves the coupling keeps on top, the wake and the bow wave. So the host reads it back: `naval-client`'s `wet_ships.rs`
copies a box about each stem (4 % of her length aft of it, 1.5 % ahead, a quarter of her patch's half-breadth to each side)
out of the ocean's heights about the hulls (`naval_ocean::hull_heights`, the very patches the wet band reads, already
`COPY_SRC`) through a ring of three buffers, and hands the highest into `BowSpray::drawn_sea` two to four frames late. The
box is placed by the stem as the last frame drew her, the pose the heights were evaluated about (the ocean's pass runs in
the render, after the effects; a sped-up theatre moves a hull metres a frame). The rise and the sheet's and runoff's easing
run over the whole presentation interval, not the particles' capped step, so a faster clock does not steepen a plunge. A
hull out of view, or the heights off, falls back on the combat sea with the bow wave's crest on it. The copies are a few
hundred bytes a hull, outside any timed pass.

**What comes aboard** (`aboard`), each frame, for every hull whether her spray is drawn or not:

- The sea's *head* on her stem over its design waterline, and how fast it climbs (*w*, eased over 0.15 s, restarted when
  its source changes).
- A plunge throws a sheet when its spray reaches over the deck edge: the water meeting the flare leaves it in a jet at
  about twice the speed the sea climbs the stem (the spray root of a wedge entering water, Wagner 1932), so it rises
  (2w)² / 2g over the head; `jet` = head + 2w²/g − freeboard. The sheet is smooth(−1.5, 1, jet) × smooth(1, 4 m/s, w)
  × smooth(2, 8 m/s, her speed), carried aft by the apparent wind along her bow (her way through the world plus the true
  wind against her: none under 4 m/s, all over 18). Running before the sea the apparent wind is near nothing and the
  encounter slow: no sheet.
- Green water comes aboard when the sea itself stands over the deck edge at the stem (the seakeeping deck-wetness
  criterion, Ochi 1964), fully 1.5 m over.
- Spray torn off the bow's sheets and blown aboard: 0.35 × the sheets' strength × the sea's weight (significant height
  1.5–6 m) × the apparent wind's carry.

Each gives the wetting, how far aft it lands (a sheet up to 30 % of her length, green water 22 %, blown spray 12 %), how high
it climbs over the deck edge (a sheet 1.5–11.5 m: the bridge face in the worst) and the green water standing on deck.

**Calibration** (`.build/native/look-wet-ships/scripts/measure.ts`: a Fletcher ordered to 20 m/s, 70 s into the battle,
20 s sampled every fourth frame, the drawn sea read back): into the North Atlantic's 22 m/s storm head on she makes 11.4 m/s
and throws a sheet on 3.0 % of frames (one every few waves), her bins after 20 s 0.50, 0.35, 0.20 from the stem and dry aft;
45° on the bow, 5.7 %; running before it, none (jet at most −1.8 m); into a 12 m/s sea, 1.7 %; in calm water at 18 m/s her
bow wave stands 3.2 m under her deck edge and nothing comes aboard. Green water never came over her 6.8 m bow in these
seas; it is there for lower freeboards and heavier seas.

**What it throws.** For the twelve bows seen largest:

- *The sheet* (`curtain`): five strands across the stem head, each the path its water takes relative to the hull, up off
  the deck (1.5–10 m over it, jittered, the middle highest) and back over the forecastle as the hull runs on under it and the
  air slows it (the side sheets' drag of 0.6 a second, keeping 55 % of the hull's way as it leaves), landing on the deck
  aft of the stem. It stands as the throw does, up within 0.1 s and falling back over 0.7 s, torn more as it falls. The
  strand batch grew by five strands a bow.
- *Billows and drops over the deck*: 45 billows (a quarter of them fine mist) and 150 streaked drops a second a side at a
  full sheet, launched from the stem head across the deck at (2·g·h)^½ for 2–10 m, outboard at most 0.6 m/s, keeping 55 %
  of the hull's way, lit and hazed as every billow and drop (`spray.wgsl`).
- *Water running off the deck* (green water, and 60 % of a sheet, draining over 2.5 s): 70 streaked drops a second a side
  pouring off the deck edge along the wetted forecastle (`BowShape::deck`, nine stations over its first 30 %: each one's
  half-breadth at the highest slice and side height), outboard at 0.6–2 m/s with the hull's way, into the sea beside her.

The web port's plunge spray (`slam`) is unchanged: it still reads the combat sea.

**Cost**: a Fletcher's bow in the storm, the transparent pass 0.31 → 0.32 ms (`ships.md`, "Wet ships", for the setup);
the aboard model is a few dozen flops a hull a frame.

**Tests**: `still_water_ships_nothing_over_the_bow`, `a_head_sea_throws_sheets_over_the_bow_and_a_following_one_does_not`,
`green_water_comes_aboard_and_runs_off_the_deck_edges` (`bow_spray.rs`).

**Open**: the sheet and its billows pass through the deck and superstructure (the depth test hides what falls behind
them; nothing splashes on the deck); the readback's box takes the highest water about the stem, so a crest beside the bow
counts as one against it.

## The sea answers what strikes it (2026-10-07)

Splashes, torpedo hits and sinking hulls drew spray, columns and foam rings, but the wave surface under them did not
move. Now what strikes the sea disturbs the dispersive wake field itself, which turns each disturbance into ring waves
spreading at deep water's speeds, as the local interactive wave simulations of Sea of Thieves or Skull and Bones do.
Visual only: combat never reads these waves.

**What the field takes** (`naval-ocean` `src/wake/impulses.rs`, `shaders/wake_field.wgsl`): an impulse is a footprint
(a point, or a capsule along a keel) and what it does there over its duration: a height laid as a Mexican hat,
η₀ = −D (1 − s) e^−s at s = r²/R² (a crater for D > 0, a dome for D < 0; no net volume about a point, so it carries no
long-wave pedestal), raised smoothly (smoothstep) in the state's height and its previous step alike, so it has no
velocity of its own; a pressure head held under a Gaussian with a sin² envelope (negative lifts the sea, as a hull's
positive head presses it); and white water laid over e^−s (the field's foam, 1 dense). The step pass takes up to 16
a step (more wait a step, their course shifted whole, those under way first), at the field's own 30 Hz clock. Off the
field (1,536 m about the focused hull) nothing is laid. `WakeField::add_impulse`, `WakeField::strike`.

**How big** (`strike_energy`, `crater_size`). The wave energy is 2 % of the energy released at or near the surface:
the linear crater of Kranzer and Keller, fitted to measured explosion wave trains, holds the waves' energy as its own
potential energy, about 2 % of a near-surface charge's (Le Méhauté and Wang, *Water Waves Generated by Underwater
Explosion*, 1996, from the US Navy's HYDRA analyses; the same linear theory came out about 40 % high against basin
tests). The same share is taken for impacts, which is less certain. The hat holds ½ρg∫η² dA = π ρ g D² R² / 8; it is
half as deep as it is wide, at most 4 m deep (wider beyond, keeping its energy: half the field's 8 m bound), and no
narrower than 1.5 of the field's cells (shallower then, keeping its energy, so the rings far off keep their height while
the grid carries their length). It opens over √(R/g), a quarter of the period of the waves it sheds.

| Strike | Energy source | Waves | Crater R × D (High) |
| --- | --- | --- | --- |
| 5-inch round entering at 400 m/s | ½ m v² × 0.44 (naval-sim keeps 0.75 of the speed at entry), m = 13,800 d³ kg (half the cube of the calibre in inches, in pounds) | 20 kJ | 4.5 × 0.5 m (floored) |
| 5-inch HE burster, 3.3 kg at the surface | kg TNT × 4.184 MJ | 0.28 MJ | 4.5 × 1.9 m |
| 16-inch AP round at 460 m/s | as the 5-inch | 0.86 MJ | 5.4 × 2.7 m |
| 16-inch AP burster, 18.5 kg 2.5 m down | × e^(−depth / bubble radius), the bubble 3.4 (W / (z + 10))^⅓ m (Cole) | 0.81 MJ | 5.4 × 2.7 m |
| 16-inch HE burster, 70 kg at the surface | | 5.8 MJ | 9.6 × 4 m |
| 533 mm torpedo, 350 kg 4 m down | warhead 350 (d / 533 mm)³ kg | 20 MJ | 17.6 × 4 m |
| Aircraft, 4 t at 60 m/s | ½ m v² | 0.14 MJ | 4.5 × 1.3 m |
| Depth charge, 130 kg 30 m (15 m) down | a dome where the charge is deeper than its bubble | 28 kJ (0.86 MJ) | dome 4.5 × 0.5 m (5.4 × 2.7) |
| Magazine beside the water, 2 t at 6 m | most of a magazine vents into the air: 2 t taken | 117 MJ | 43 × 4 m |

The events (`naval-client` `src/sea_strikes.rs`, wired in `effects.rs` beside the wakes, each taken once by sequence):
`splash` (the round's calibre and its velocity at entry), `burst` with `water_burst_y` (a burster in the water; its
charge from the blast radius, 3 ∛kg m, naval-sim's `burst.rs`; its depth below the sea there), `torpedo-hit` (by
diameter, at the hit's depth), an aerial torpedo's `water_entry`, `depth-charge-blast` (130 kg), `aircraft-crash` over
water (its velocity, else 60 m/s), and a `module` detonation within 15 m of the surface (a magazine). The splash's
column rises at its event (`splash.rs` throws its jets at once, at most 0.1 s late), so the crater opens under it.
naval-sim emits no mine hits; bombs are rounds and bursters as shells are.

**The rings are deep water's.** The field steps ∂²h/∂t² = −g √(−∇²) h, ω² = g|k|, with `kernel.rs`'s fitted √(−∇²),
within 4 % of |k| from 6 cells to 12 × 2^levels cells: on High (512 cells of 3 m, 4 levels) 18 to 576 m, on Medium (6 m
cells, 3 levels) 36 to 576 m, on Ultra (1.5 m, 5 levels) 9 to 576 m. The hat's spectrum peaks at k = 2/R, waves πR
long: 17 m for a 16-inch round, 30 m for an HE burster, 55 m for a torpedo (phase speeds √(gλ/2π) of 5.2, 6.8 and 9.3
m/s, group speeds half that). A crater's train is dispersive, longest first: at distance r after t seconds the local
wavenumber is k = g t² / 4r² (the Cauchy–Poisson problem's stationary phase). `tests/wake.rs`
(`a_crater_rings_the_sea_with_deep_water_dispersion`) lays a 6 × 2 m crater and reads the zero crossings 25 s later:
24, 36, 60 and 78 m waves at 81, 96, 120 and 154 m, where deep water has 26.7, 37.6, 58.8 and 97.5 m (the last at
the train's thin leading edge). Rings shorter than 6 cells are slowed and damped by the kernel's roll-off: on Medium
the shortest rings are about 36 m whatever struck. The field's friction (0.065 /s, the web's, which keeps the wakes
local) halves the rings' height in 21 s, far faster than water's viscosity would (days for these lengths).

**Breaking.** The field's breaking foam uses a threshold tuned for the hulls' under-resolved waves (a slope of 0.015
over 12 m) times the fastest nearby hull's speed share, so a strike's rings passing a fast hull would have turned
white for 200 m. Each impulse opens a ring train instead (up to 16, the newest kept): its footprint grown at √(gR/2),
the group speed of the waves at k = 1/(2R), for 20 s after it last acts, fading over the last 5 s; inside it the hulls'
breaking is not taken, and the strike's own white water (laid over the crater, decaying over the field's 9 s and
spreading 3 m²/s) is the rings' breaking. A hull's shoulder foam near a fresh splash fades for those seconds. Since
2026-10-10 the threshold is taken only along the hulls' own sides, so rings outliving their train never break either
("Splash rings that broke white").

**Foundering hulls** (`Sinkings`). naval-sim takes a flooding hull down by her level long before it calls her lost:
the Bismarck of `bismarck;;iowa:hard,iowa:hard,iowa:hard` (10 km, seed 1941, her guns held) settles past 1.5 m at
about 200 s, is 5 m down at 240 s, capsizes and goes down by the head with her stern in the air (her origin 11 m down
at 330 s, 49 m at 410 s), and only at 441 s, 79 m down, does `sinking` turn true; her wreck stays drawn on the bottom.
So a hull founders when her level, averaged over 8 s from her waterline so the sea's heave (or a first sight in a
trough) does not count, is more than 1.5 m down, or once she is lost; her air pours out at full strength once she has
settled another 3 % of her length (deck awash). While she
settles the sea lifts over her keel (a held head of −0.3 m × √(her size against a 250 × 36 m hull), overlapping 2 s
pulses every second, whose sin² envelopes sum to a steady lift, with lace over it), and air boils up along her as a
Poisson process of 1.5 a second at full strength: each a dome as high as a bubble plume's fountain, 1.3 (g Q)^⅔ / g for
a flux Q log-uniform over 0.5–8 m³/s × her size (0.4 to 2 m: the plume's velocity at the surface, 1.6 (g Q)^⅓, squared
over 2g), 0.12–0.25 of her beam across, white water over it. She has gone under when her highest end and her upper
works are (her origin's depth less L (½ |sin trim| + 0.15 cos trim), 0.15 L the upper works' height), or when she is
no longer drawn: the sea heaps over her once (−1 m × √size over 6 s, white water along her), and boils keep coming at
3 a second, dying away over 35 s; after 150 s the sea is quiet, though her wreck may still be drawn. A wreck first
seen under (a scene loaded over it) stirs nothing, and a hull that founders for 600 s without going under (on a
shallow bottom) has spent her air. A submarine's dive is not foundering: she boils only once she is lost.

**Shown where.** Everywhere the wake field runs: Medium, High and Ultra's ocean (none on Low, which has no field), in
battles, theatre battles at the helm and the cruise scene alike; the field follows the focused hull, so a strike more
than 768 m from her (an enemy's splashes in a long-range action) leaves no rings. The hulls' wet band, the underwater
probe and the rain's splashes read the field's height (`wake_height`), so a hull beside a strike or over a foundering
keel takes the water as drawn.

**Cost.** None while nothing strikes the sea: the step's two loops run zero times, the uniform grows from 576 to 1,616
bytes a step (slots of 1,792; 1,872 in slots of 2,048 since the hulls' breaking stretches), and no sampler or binding is added to the sea's pipeline (the surface reads the same
display textures). With sixteen impulses and sixteen ring trains in every step, 30-step batches timed to completion on
the GPU (Apple M5 Pro, Metal, load average 5–7, three runs) took 0.30 ms a step on High's 512² either way and 0.97–0.98
ms on Ultra's 1,024²: the loops are a few ALU operations a texel in a pass bound by its texture reads, below what the
timing resolves. In the client (dev-client, 1600 × 900, the salvo below from 520 m up, six alternating runs), the pass
`ocean wake step` took 0.25–0.42 ms on both builds, before and through the salvo. CPU: a few vector pushes per event and
per foundering hull a frame.

**Seen** (`.build/native/look-sea-ripples/`, `ripples.ts` scouts a battle, replays it to each moment and draws it from a
low and a high camera with this build and master's; `pair-*.png` are before | after | difference ×4): a 16-inch AP
salvo falling 140–700 m about a Bismarck (`bismarck;;iowa:normal`, 14 km, seed 1941, the salvo at tick 1960), at the
map's 9 m/s and at 3 m/s; a 533 mm torpedo hit on her bow (`bismarck;;fletcher:hard,fletcher:hard,fletcher:hard`,
6 km, tick 21543); her foundering under three Iowas (above). At 3 m/s the rings show in the sun's glitter from 520 m
up, concentric inside each splash's foam ring and 40–60 m across at 12 s; in the 9 m/s sea (2.4 m significant height,
an rms slope near 0.2) rings of 0.1–0.3 m are a faint change in the glitter and foam about each column, as they would
be at sea. The torpedo's 55 m rings move her wet band along her side and, 20 s on, the whole sea in the foreground
(13 % of the frame's pixels change by more than 8 levels, none of it a visible pattern). Foundering, white water boils
up along her as she goes down by the head. Stills with no event (`--camera broadside` and from 600 m) draw master's
bytes.

**Open.** The rings are real-sized, so most of them are hidden in a seaway. Rings stop at the field's edge (768 m about
the focused hull). A hull is no obstacle to them (they pass under her as the hulls' own waves do). Impact energy shares
are less certain than explosions'. No mine events exist in naval-sim.

## Aerial torpedo drops (2026-10-07)

The simulation marks both successful entries and drops that break up with `TorpedoEffect.waterEntry`, keeping the
entry velocity for the small directed splash (`WaterEventKind::TorpedoEntry`) and foam ring. The column is scaled
to 0.025 of a heavy shell's splash. Ship-tube launches keep their existing presentation.

The torpedo track field follows the water under the view and a round near the sight line at every zoom. Previously
only a magnified lens followed a distant round, so zooming out could centre the 1,536 m field on the camera and drop
the visible wake. Moving the camera while paused repositions the field without ageing the water.

## The wake since the port (2026-10-03)

Since 2026-10-07 every hull keeps its sampled course even without one of the atlas's drawing slots. Changing the
focused ship assigns a slot to her existing trail instead of starting it from scratch. The theatre map continues
the CPU histories while hiding the scene; a rebuilt scene takes those histories in its new local coordinates.
Fast theatre frames allow the distance a ship can actually travel before rejecting a pose as a teleport.
The atlas remains the same size. `trail::tests::changing_atlas_slots_keeps_the_unwatched_ships_course_history`
and the client's `tests/effect_history.rs` cover the continuity.

The browser game's realistic wake drew one band of dense white water a beam wide, painted to its edge, that faded as a
whole. Real wakes (aerial and stern photographs) stay white only behind the stern; further astern the white water draws
out along the track into a braided rope of streaks down the middle with sparse lace on either side and a thin line
along each edge, over turquoise water lit by the bubble cloud, and a faint line of foam lasts as long as the slick.
Native-only changes:

- **Where a point lies across its band.** Each band stamp records its centre's offset from the band's centreline and its
  half-width there (`Stamp::across`; `NO_BAND` for splash rings and torpedo tracks), and paints how central each point
  is into the atlas's blue (churned water) or alpha (slick), now `Rgba8Unorm` (`stamps.wgsl`, `TRACK_RANGE`,
  `TRACK_ZERO`). Stamps write it past their extent (a quarter of it, and at least 4.5 m and a texel since 2026-10-05) so
  it holds to the band's edge. Every stamp of a band agrees on it at a point; where bands overlap (a ship crossing her
  own trail) max blending keeps the most central, and filtering toward an empty texel reads as the edge. The tile rows
  carry each hull's beam (`slick_center.z`).
- **Old water streaks along the track** (`wake.wgsl` `trail_read`, `wake_streaks`): streak noise fine across the band
  (16 per half-width) and drawn out along it (0.7 beams), braided by warps at two scales, read where the point lies,
  not through the warp that frays the outline. Its cover: 0.6 in a core out to 0.32 of the half-width, 0.1 across the
  rest, 0.55 in a line at 0.9; the band's edges wander ±25 %. "Young" water (turbulence 0.45–0.85, about the first
  150–400 m at full speed) keeps the eddies, a little streaked, thinning to half toward the sides. Old water is lace
  (0.7 of its cover at 0.75 opacity, `churned_water`'s `young`), young water the dense white water as before.
- **The bubble cloud** fills the band, densest in the middle and patchy like the foam, fading over its outer third; it
  is brighter and more turquoise (`BUBBLE_STRENGTH` 0.75, `BUBBLE_ALBEDO` 0.42, `AERATED_MATTE` 0.6).
- **The slick's middle** keeps a line of foam streaks (cover 0.16 within 0.22 of its half-width) and a residue of
  bubbles (0.2, was 0.08) for its four minutes; its edges wander and fade over the outer half instead of stopping.
- **Lifetimes**: churned water e-folds in 30 s (was 24) and lasts 90 s (was 55), fading from 60 s.
- **No foam at zero cover** (`torn_foam`): a threshold at 1 − cover with a fixed tear let the noise's peaks through
  where the cover was nothing, which drew white crescents beside the wake.

Cost: whole frames at 1600 × 900 over a Bismarck's wake, old and new shaders swapped by hot reload in alternating
rounds, medians −0.02 to +0.45 ms across three views (Apple M5 Pro, load average 35; within the run's noise).

Since (2026-10-04), against renders where the wake was a saturated teal band with a hard outer edge and painted streaks:

- **Lace, not a mask**: the churned water's mask tears over at least 0.3 of its pattern (`WAKE_TEAR`; the `tear` knob
  is 0.1), and churned water thins with its energy (`CHURN_THIN`: 0.4 opaque where it is nearly gone), so the band
  frays into the sea. Streaks are shorter and wider (9 per half-width, 0.45 beams long), the edge lines fainter (cover
  0.3) and the outline wanders ±40 % at two scales. Old churned water is sparser lace (0.62 cover, 0.55 opaque).
- **Pale turquoise close astern only**: the bubble cloud thins from 0.3 of the half-width to the edge and keeps 40 % of
  itself once the water is old; the cloud's colour is paler (`foam.wgsl` `BUBBLE_PATH` 2.6 m).
- **Splash patches** (no band) are dense white water only above 0.7 turbulence (`SPLASH_YOUNG`); elsewhere lace (since
  2026-10-08 a soft patch, not a ring: "Splash clouds and plumes").
- **The waterline**: the surface's shoreline foam, which drew a crisp white stripe along every hull, mostly gives way
  beside a hull to `hull_foam` (`ocean.md`, "Foam that reads as foam").

Since (2026-10-05), against a wake that came out behind the stern in jerks and showed straight-edged patches:

- **The band keeps up with the stern.** Samples are laid every 3 m of travel and the trail painted at 20 Hz, so the
  band's end fell up to a sample and a repaint behind the stern and jumped after her: a destroyer at full speed (25 m/s
  over the water at the battle's pace, a sample every eighth of a second) left her transom bare and white again about
  eight times a second (the churned water at her stern swung from 0 to 0.95). Every rasterisation now paints a head,
  churn and slick, where the hull stands (`WakeFoam::head`), and the stern keeps over 0.75 of its coverage
  (`the_band_keeps_up_with_the_stern`).
- **No straight cuts.** The surface reads the band's coverage through a warp of up to about 3.75 m but where a point
  lies across the band where it is, and a band's stamps wrote that coordinate only a quarter of their extent past
  their coverage. Where the warp reached further (a destroyer's band is 1–2 texels of 6 m across), the coordinate was
  missing and the pixel was shaded as a splash: dense white water and a full bubble cloud, cut along the atlas's
  texels. A slick read where its coordinate was missing stilled the waves at full strength out to a 16 m texel's edge.
  Band stamps now write it at least 4.5 m and a texel past their coverage (`stamps.rs`, `TRACK_REACH`), coverage read
  where no band says where it lies counts as outside the band, and a slick there as past its edge.

## Bow waves in proportion (2026-10-05)

The Kelvin V's amplitude (`BowSlot` wave.y) was `kelvin` (1.6) × U²/2g × √(B/L): speed alone, nothing of the hull's
size. A Fletcher at 36 kn (Froude 0.55) drew a V of ±10–12 m about a 3.8 m draft and 6 m of freeboard: the divergent
waves heaped the sea over her forecastle 0.13 L aft of the stem (+8 m on her side) and the transverse trough bared her
bottom at 0.68 L (−5 m). It is now `kelvin` (0.9) × the stem crest (`stem_crest`, which already scales by beam and caps
at 4 m), so every hull keeps the crest's proportion. Measured with a CPU mirror of the vertex stage (the hull side
0.3 m out, then the sea within 3 L astern and 1 L abeam):

| Hull at full speed | Hull side, highest / lowest (m) | V about her (m) | Draft (m) |
| --- | --- | --- | --- |
| Fletcher, 18.5 m/s | +8.0 / −5.2 → +3.7 (the stem) / −1.3 | +11.9 −9.1 → +3.3 −2.1 | 3.8 |
| Gleaves, 19.3 m/s | +8.8 / −5.7 → +4.0 / −1.4 | +13.2 −10.0 → +3.7 −2.4 | 3.9 |
| Atlanta, 16.7 m/s | +4.5 / −4.3 → +3.4 / −1.3 | +5.0 −6.1 → +1.6 −1.5 | 6.3 |
| Bismarck, 15.4 m/s | +6.2 / −3.8 → +6.2 / −1.9 | +3.4 −4.0 → +1.7 −1.8 | 9.3 |

The stem crest itself is unchanged (a Fletcher's 3.7 m with the heap is about what a fine bow raises at 36 kn). The
surface still shades the V, its slopes and the white water on its crests, at the old amplitude (`kelvin_shading`, 1.6 ×
U²/2g × √(B/L), carried in the slot's fourth vec4): long and low at its true height, the V did not read from astern at
all (`parity-wake-astern`, a Fletcher at 20 kn, lost it entirely; its reference was blessed again for the lower
transverse trough). Only the drawn height, which the hull meets, keeps the crest's proportion. The bow spray's surges
stop throwing sheets over a destroyer's bridge, since the sea no longer rose 8 m beside her stem.

## More visible bow white water (2026-10-06)

The stem's white water used a 0.4–1.2 m crest-height gate and a high noise threshold. A Fletcher at 8 m/s raised a
0.44 m crest but drew almost no breaking foam. The bow's white water now follows the crest's steepness, with a separate
height gate keeping slow hulls quiet. The visual aim is the spreading, broken white water visible in the Navy's
[McCampbell underway photograph](https://www.dvidshub.net/image/6143119/uss-mccampbell-aerial-shots).

- The foam spreads farther outboard and lasts longer along the bow (`BowWaveTuning`: foam 1.15, length 2.8, spread
  0.12, side wash 0.7). It fades before the analytic bow region's bounds, keeping the edge soft. (Pulled back in
  to the crest on 2026-10-08: "Less white water on the V, a tighter bow".)
- The world-anchored strands retain more of the thinning foam instead of discarding it at the noise threshold.
- `BowWaveSample` carries a faint bubble cloud beneath and just beyond the foam, combined with the trail's bubbles
  in `wake_surface`; the cloud has the same moving breakup as the white water. (Since 2026-10-08 it lies only under
  the white water.)

The crest's height, the Kelvin V's height and the spray's launch height keep the proportions above. (The crest's place
and shape, its white water and the sheets changed on 2026-10-12: "A bow wave that climbs the hull".)

Verified in the native client from the bow quarter and a low camera: Fletcher at 2.6, 9.6 and 17.0 m/s and Bismarck
at 13.5 m/s in a 5 m/s wind, plus Fletcher at 11.5 m/s in a 12 m/s wind under overcast skies. The 76 ocean unit tests
(including shader composition and bow-height proportions) and `parity-wake-astern` pass. Captures: `.build/native/bow-wave/`.

## A closer, longer Kelvin crest train (2026-10-06)

From above, the V showed a few isolated whitecaps with large gaps between them. The visual reference for this pass is
the user's aerial container-ship photograph: a feathered train of broken crests along both arms, extending well astern.
`BowWaveTuning::kelvin_wavelength` now gives the V its own spacing (0.3 of the speed-derived wavelength), while
`speed_scale` still controls the stem crest's decay and the bow spray. This is an artistic calibration: the physical
deep-water wavelength varies with speed squared ([Rabaud and Moisy](https://arxiv.org/abs/1304.2653)), and the photo's
ship speed is unknown.

The V reaches 4.5 hull lengths from the stem instead of 2.6, with the foam's decay length increased from 2 to 3.5 hull
lengths (1 since 2026-10-08, when the crests stopped breaking along the arms: the next section). The analytic source depth is 0.12 beam instead of 0.25 so it does not suppress the shorter crests. Height,
slopes and crest foam all read the same wavelength; the existing pixel/mesh filtering, curved track, cusp angle and
soft outer fade still apply. The stem's height, decay and spray launch shape keep their previous tuning.

Verified against an isolated `2f834d25dc17` baseline in overhead and astern views: Fletcher at 9.9 and Bismarck at
13.8 m/s, plus Fletcher at 2.7 and 18.1 m/s and in a 12 m/s wind at 11.5 m/s. The 83 ocean tests and Clippy pass.
`parity-wake-astern` retains its wake-visibility assertion; its image reference was refreshed for the closer,
longer crest train. Captures and logs: `.build/native/kelvin-crests/`.

## Less white water on the V, a tighter bow (2026-10-08)

The owner's references (a carrier group from altitude, an Iowa and a Midway from low over the water, a battleship, an
oiler and a carrier from directly above) show the Kelvin arms as thin dark ripple lines with no foam on them, and at
the bow a narrow white strip hugging the hull. The client broke every divergent crest near the cusp in white for three
hull lengths astern, and paled the sea far outboard of the bow.

- **The arms.** The crests' white water came only from `crest_foam` in `wake.wgsl` (the dispersive wake field adds none
  there: switching the analytic terms off left the arms clear). Its steepness gate read the slope the surface is
  shaded at (`kelvin_shading`, 4.2 times the drawn height for a Fletcher, 2 for an Iowa), so it was always open. It
  now reads the drawn wave's steepness (ak), opening at `kelvin_steep` 0.06 and full at 3.3 times that, fades over 1
  hull length instead of 3.5, and is fainter (`kelvin_foam` 0.7). Only a fast hull's first crests break.
- **The bow.** The paler area was the bubble cloud: 0.3 of a band 1.5 times as wide as the white water's. It now
  lies only under the white water, at 0.22 of it. The crest's white water stays near
  the crest (`foam_length` 1.3 of the crest's decay lengths instead of 2.8, `foam_spread` 0.05 m per m instead of 0.12,
  a band 1.1 instead of 1.3 times the crest's width) and is gone by 0.4 of her length instead of 0.8.
- **Kept:** the wash along the hull's sides (`wash` 0.7, `wash_width` 3.5 m, out to 0.75 L), which the owner likes,
  and the stem's own white water.

Measured with a CPU mirror of the fragment stage before its noise (white water over 0.2, the paler sea where the
bubbles pass 0.1; scratch, not in the tree):

| Hull | White water (m²) | Paler sea (m²) and its reach out from the centreline | Crest foam on the V |
| --- | --- | --- | --- |
| Fletcher, 17.0 m/s | 1,319 → 852 | 1,960 → 902, 22 → 13 m | 843 m² to 2.7 L → 7 m² within 0.6 L |
| Fletcher, 9.6 m/s | 372 → 151 | 1,187 → 354, 17 → 10 m | 791 m² → none |
| Iowa, 14.6 m/s | 3,000 → 2,024 | 5,891 → 2,301, 43 → 24 m | 4,414 m² to 3.1 L → 14 m² within 0.2 L |
| Bismarck, 14.0 m/s | 2,888 → 1,965 | 5,559 → 2,183, 43 → 24 m | 3,716 m² → none |

From astern a fast destroyer's V still shows bright bands and grainy grey strips on its crests: the slopes shaded at
`kelvin_shading` and the sun's glitter on their steep faces, not foam (a render with every wake's foam and bubbles off
shows them unchanged). `kelvin_shading` is the knob if the arms should read fainter.

Seen in the native client against master's build: an Iowa at 14.6 m/s and a Fletcher at 17.0 and 9.6 m/s, from
380 m up ahead, overhead the bow, low on the bow quarter, close on the bow and from astern, in a 5 m/s wind; and the
port's Bismarck. The 107 ocean tests pass; `parity-wake-astern` was blessed again (its old reference was already 4.8
levels off on master; this change moves it 0.9, the white dashes on the arms). `chase-one-ship`, `fleet-measure30`,
`port-bismarck` and `theatre-cruise` fail on master's build by the same margins (stale references; `port-bismarck`
moves 1.1 levels nearer its reference here). Captures: `.build/native/wake-tune/`.

## A bow wave that climbs the hull (2026-10-12)

The owner's reference (a Nimitz-class carrier at speed from her port bow): white water starts at the stem, a thin
bright ribbon hugs the side and peels away, and the sea ahead of the bow is undisturbed. The client drew three things
that photograph does not show:

- **White water ahead of the bow.** The crest's ramp began 0.04 of the hull's length ahead of the stem and the heap
  about it was a Gaussian centred 0.02 L aft, 0.07 L long and half a beam wide: a mound that stood 15 to 20 m ahead of a
  battleship's stem and as far abeam, with the stem's white water (a Gaussian about the stem, fore and aft alike) on
  top of it. The stem itself was placed 0.49 L ahead of the origin, the end of her length: 0.4 m ahead of a Fletcher's
  stem at the waterline, 2.7 to 3.9 m ahead of Enterprise's, Yamato's, Iowa's and Bismarck's, under their bows' rake.
- **A fountain.** The sheets left that mound, already at its full height at the stem, upward at 0.45 of the hull's speed
  and outboard at 0.55: 2 m over the crest and 8 m out, over a destroyer's hull number.
- **A mound, not a wave.** The crest was a Gaussian ridge 0.12 beam + 1.5 m in half width (6.2 m for Yamato) on a
  parabolic entrance twice as full near the stem as a battleship's.

What draws it now (`bow_waves.rs`, `shaders/wake.wgsl` `bow_waves`, `bow_spray.rs`):

- **Nothing ahead of the stem.** The crest climbs the side from the stem (0.15 of the peak's distance ahead of it, so
  the surface meets the stem without a step) and stands highest `crest_peak` aft of it: 1.1 U²/g ÷ (1 + U/√(g·draft))
  ([Noblesse et al. 2008](https://doi.org/10.1017/S0022112008000220), "Simple analytical relations for ship bow
  waves"), 7 to 9 m for a battleship at 25 kn and a destroyer at 30. The heap (`hump`, 0.7 → 0.3 of the crest) is the
  water standing against the bow's sides: 0.2 beam + 1 m out from the side, not half a beam about the centreline. The
  white water starts at the stem.
- **The hull's own waterline.** The stem is the foremost point of the sliced waterline (`bow_waves::waterline_stem`),
  and the crest follows the hull's half-breadths an eighth, a quarter, a half and the whole of 0.3 L aft of it
  (`bow_waves::entrance`, a fifth vec4 a slot: the `Wake` uniform is 39,664 bytes). Yamato's are 2.6, 4.7, 9.3 and
  15.8 m against the parabola's 4.6, 8.5, 14.6 and 19.5; Iowa's 1.9, 3.2, 6.5 and 13.1 against 3.9, 7.2, 12.4 and 16.5.
- **A crest with a face.** Half width 0.05 beam + 0.8 m (2.7 m for Yamato, 1.4 m for a Fletcher), its inboard side 2.2
  times as broad as its outer face, a trough inboard of that. The vertex stage draws it no narrower than 1.6 cells of
  the water mesh.
- **White water with an age.** A ribbon along the crest's outer face where the sheet plunges (`foam_length` 2.5 crest
  decay lengths, gone by 0.5 L), the sheet's root against the side from the stem to the peak, and what the breaker left
  on the water the crest has passed, which fades over 0.16 L downstream: the farther inboard of the crest line, the
  longer ago. That residue and the wash along the side are drawn as part lace (`BowWaveSample::young` 0.7, read by
  `churned_water`) and torn into streaks along the flow; the noise's clumps are gone.
- **Sheets that climb only as far as the crest.** A strand leaves the crest line 1.1 times as high as the crest there,
  climbing at the speed that carries it to the crest's peak (`Bow::rise_at`: √(2g × what is left to climb), nothing
  from the peak aft) and outboard at 0.45 of the hull's speed, a little faster than the crest line leaves her track.
  A sea met adds up to 0.2 of her speed upward and a plunge 0.55, so a bow driving into a sea still throws it up.
  Tufts are 0.6 the size.

**A slicer bug found on the way.** `hull_slice.rs` took a triangle for a level only when it crossed it. A hull modelled
with an edge loop on the design waterline has none: Yamato's waterline slice held 7 of 30 stations, so she had no
contact foam along most of her side and her shelter read nothing there. Each level is now sliced 1 mm over its height
(`LEVEL_NUDGE`; `an_edge_loop_on_the_waterline_still_slices`).

**What a bow ships.** The readback of the drawn sea about a stem (`wet_ships.rs`) now reads 0.08 L aft of it (0.04
before), so the box holds the crest's peak. The crest with the water against the side is 1.27 times the stem crest
where it was 1.65 at the stem itself: a Fletcher's at 18.5 m/s stands 3.0 m, not 3.9 m. The over-the-bow calibration above
was measured on the taller one and has not been run again.

Seen in the native client against master's build, in the default seaway at 7 m/s of wind: Yamato at 12.7 m/s, a
Fletcher at 15.2 and Enterprise at 14.8, each from high on the port bow, the whole ship from the bow quarter, overhead,
low on the bow, abeam of the bow, ahead and the stern quarter. Not measured: the frame's cost (the fragment stage
trades one Gaussian for a four-branch breadth lookup).

## Kelvin arms as thin lines, kilometres long (2026-10-13)

The owner's reference, a carrier group from some 500 m up, shows each hull's V as two thin dark lines of short crests
in echelon, a few white flecks near the bow, quiet water between them, running many hull lengths astern until they
cross the next ship's. The client drew a fan of evenly spaced ripples filling the V for a hull length or two and
nothing past 4.5 lengths (a Fletcher's arms ended 510 m astern), and the far end of the V swung with every yaw.

- **The cusp line.** `bow_waves` in `wake.wgsl` takes how far inside the 19.47° line a point lies as the Airy
  function's argument, ζ = 1.96 (k₀·run)^(2/3) (1/√8 − t): the two systems' phase difference, linearised about the cusp
  (`CUSP_ZETA`). Outside the line the crest dies away as Ai does (exp(−⅔ x^(3/2) − x/4)), within metres at any range,
  where a fixed angular fade (t² from 0.095 to 0.125) used to stand: 46 m wide 1 km astern. Inside it the first fringe
  keeps 1.15 of the V's shaded slope (`CUSP_LINE_GAIN`) and the fringes within it 0.15
  (`BowWaveTuning::kelvin_interior`, in the knob the shader no longer reads the reach from). The drawn height, which
  the hull meets, keeps the interior. The band is some 20 m wide 300 m astern of a hull at 15 m/s and 40 m wide 2 km
  astern.
- **Shorter crests.** `kelvin_wavelength` 0.3 → 0.2 of the speed-derived wavelength: denser dashes along each arm.
- **As far as her track follows the arc.** The V reaches 12 hull lengths (`reach`, was 4.5) where her track allows.
  `BowWaves` keeps each hull's track a point every 20 m and draws the V no farther than two thirds of the way back to
  the first kept point that strays from the arc it is laid on by more than 4 % of the run and half her beam (the cusp
  `run` astern was raised 1.5 run back along her track: its waves leave 35.26° off her heading at their group
  velocity). In a turn the arms shrink, eased over a second, and grow again along the new track instead of swinging
  a kilometre with the helm. A track not yet kept (a new hull, a jump of 100 m) counts as following it.
- **The arc from the track, not the heading.** The V's curvature was her heading's change over distance, smoothed over
  1.2 s; a destroyer's yaw in the default seaway made it wander by ±0.0006 per metre, which moved a 4.5-length arm's end
  by tens of metres a second. It is now the circle through where she is and her kept track 0.4 and 0.8 of her length
  back (`fitted_bend`), smoothed the same, and the V leaves the stem along her course over the ground (`course`, a sixth
  vec4 a slot: the `Wake` uniform is 39,792 bytes), not her heading, which yaws and drifts in a turn: 5° between them
  put the end of a 1.4 km arm 120 m off her track. The bow's crest keeps her heading. The V's bounds are tested along
  its own arc, so the inner arm of a hull in a long turn is not cut off by a box drawn about her heading.
- **Far arms.** The V's crests are drawn down to three pixels (`KELVIN_FINEST`; five, `finest`, before): they stand
  still in a view that follows the hull. The slope variance of crests too short to draw roughens the sea instead
  (`BowWaveSample::variance`, `KELVIN_ROUGHNESS` 0.12, patchy along the arm over 180 m; `surface.wgsl` adds it to the
  reflection's spread as it does the rain's): a far arm reads as a line of rougher water, darker from aloft at noon.
- **Cost.** Past two to four hull lengths astern only the two bands are evaluated (ζ under 6 to 9,
  `CUSP_INTERIOR_FROM`, `CUSP_INTERIOR_TO`), so a long V costs two strips of sea, not the whole fan. Measured at
  1920 × 1080 from 420 m up ahead of the carrier group and 90 m up on her quarter (GPU busy, medians of three
  alternating rounds, one binary per variant, Apple M5 Pro shared with other sessions): no bow waves 7.79 / 7.57 ms,
  the V cut at 4.5 lengths 8.14 / 7.91, at 12 lengths 8.15 / 7.91.

Seen in the native client against master's build, in the default seaway at 5 m/s of wind, noon: Enterprise with a
Fletcher, an Atlanta and a Gleaves in company at 15 m/s from 420 m up ahead (the reference's view), from 90 m up on the
quarter and from the chase camera; a Fletcher alone at 15 m/s from 420 m up on the bow quarter, 60 m up astern and the
chase camera; and a Fletcher 25 s into a hard turn. `parity-wake-astern` was blessed again: it was already 7.0 levels
off its reference on master and this change moves it to 7.4 (the rings across her track gone, the arms with flecks).
Captures: `.build/native/kelvin-photo/`.

The same day the arms were made much fainter at Bill's request: `kelvin_shading` 1.6 → 0.7, the V's shaded slopes 0.44
of what they were and its unresolved roughness, which goes as their square, a fifth. From aloft each ship keeps two
faint thin lines; from 90 m up on the quarter the carrier's arm is barely there; close astern of a fast destroyer the
ribs are softer but still read. The drawn height, which the hull meets and whose steepness breaks the first crests, is
unchanged.

## Run it

```sh
cd native
cargo test -p naval-ocean -p naval-water-effects      # wake field vs the web's diagnostic, kernels, trails, playback, shaders
# web half (repo root, after `bun run bootstrap`): real game frames plus the exact inputs its wakes and spray consumed
bun native/crates/naval-water-effects/web/water-stills.ts [--only wake-stern,splash] [--hullsea off]
cargo run -p naval-water-effects --example water_stills --release [-- --only wake-stern] [--frames 150]
cargo run -p naval-water-effects --example record --release [-- --seconds 240 --seed 7]
cargo run -p naval-water-effects --example battle_sequence --release [-- --seed 7]
cargo run -p naval-water-effects --example splash_sequence --release   # particles alone over a flat backdrop
```

- `water_stills` replays every web scene's recording (hull poses, displayed speeds, water events and torpedoes, each
  frame's dt and camera) through `ShipWake` and `WaterEffects`, freezes the waves as the ocean stills do, and renders each
  capture through `naval_render::Renderer`: proxy hulls, the ocean with its wakes, the stand-in sky, the particles. It
  writes `native/<capture>.png`, `side-by-side/<capture>.png` (web left, native right), `diff/<capture>.png` (native −
  web, red brighter, blue darker, ×4) and `compare.json`; `--frames N` adds `timings.json`.
- `record` runs a native battle in `naval-sim-host` (a Bismarck and a Fletcher a side, every hull a bot, 9 km apart,
  9 m/s) through the host's own packet builder and writes a 10 Hz fixture (`fixtures/battle-<seed>.json`): 240 s gives
  655 splashes and torpedoes running in 1,686 frames. `battle_sequence` plays it deterministically and renders shots
  placed from the state: `battle/escort-chase.png`, `escort-bow.png`, `splash-{0.4,1.2,3.0,7.0}s.png`,
  `torpedo-track.png`, `overview.png`.

Web scenes (`web/water-stills.ts`, `waterScenes.ts`; ocean-review page, High tier, clear sky, eye adaptation off, the
waves frozen at t = 60 as the ocean stills freeze them): `wake-stern`, `wake-destroyer`, `wake-air` (Bismarck or Fletcher
from rest at full ahead, 90–180 s, 0.1 s frames through the game's manual clock), `bow-wave` and `bow-wave-ahead`,
`bow-spray-heavy` and `-ahead` (Fletcher at 25 m/s wind), `splash-0…5` (a 38 and a 20 cm splash beside a static
Bismarck, 0.2–6 s, staged at 1/60 s), `torpedo-track-0/1` (two synthetic torpedoes at 4 m depth, 25 m/s). Recordings are
byte-identical between runs. The web runs the game as it ships, the sea around hulls on (`--hullsea off` draws it
unblended); the native replay follows each capture's flag (`--uncoupled` draws it unblended whatever the web did).

## What is ported, file by file

| Native | Ports | Notes |
| --- | --- | --- |
| `naval-ocean/src/wake/kernel.rs` | `ocean/wake/kernel.ts` | The dispersion pyramid, fitted in f64 in the web's basis order; kernel.test.ts's four checks. |
| `naval-ocean/src/wake/generators.rs` | `ocean/wake/generators.ts` | Swept segments, speed gate, 0.8 s depth response, teleports. |
| `naval-ocean/src/wake/mod.rs`, `shaders/wake_field.wgsl` | `ocean/wake/WakeField.ts` | Reduce, collapse, step and display as compute on storage textures (state `Rgba32Float`, displays `Rgba16Float`, levels `R32Float`), 30 Hz fixed steps, ≤ 4 per frame, per-step uniforms in dynamic-offset slots. |
| `naval-ocean/src/trail.rs` | `WakeFoam.ts` (realistic), `FleetWakeFoam.ts` (CPU) | Samples, churned band and slick stamps, splash patches, the 8 × 8 tile atlas and each tile's painted boxes. The three translucent streams `realism.wake` off draws are not ported (the game ships with it on). |
| `naval-ocean/src/stamps.rs`, `shaders/stamps.wgsl` | `WakeFoamGpu.ts` | Texel-aligned instanced quads, `max` blending into `Rgba8Unorm` (trails: coverage and where each stamp lies across its band) or `R8Unorm` (torpedoes). |
| `naval-ocean/src/torpedo_tracks.rs` | `TorpedoTrackFoam.ts` (CPU) | Exhaust surfacing delay, drift, expiry, the view-anchored 1,536 m field at 1,024². |
| `naval-ocean/src/bow_waves.rs` | `BowWaves.ts` (CPU) | Slots for the nearest eight hulls, smoothed speed, track curvature, `stem_crest`. |
| `naval-ocean/src/hull_foam.rs` | `HullContactFoam.ts` (CPU) | Slots, world-to-hull rows of the drawn pose, the profile's breadths and side heights, the drive. |
| `naval-ocean/src/hull_profile.rs` | `hullWaterlineProfile.ts` (layout) | `WaterlineProfile`, and `SimpleHull`, the proxy hull sampled into it. |
| `naval-ocean/src/ship_wake.rs` | `ShipWake.ts` | Generators for the nearest eight hulls within 900 m, event dedupe by sequence, the uniform packing. |
| `naval-ocean/shaders/wake.wgsl` | `WakeField` sampler, `BowWaves.evaluate`, `FleetWakeFoam.evaluate`, `TorpedoTrackFoam.sample`, `HullContactFoam.evaluate` and `evaluateShelter`, `surface/shelter.ts` | The surface's `WakeSampler`: `wake_height` (vertex), `wake_surface` (normal, foam, bubbles, slick), `hull_shelter` with `shelter_light`/`shelter_reflect`. |
| `naval-ocean/shaders/mx_noise.wgsl` | three's `mx_noise.js` (`mx_noise_float(vec3)`) | MaterialX Perlin noise with its lookup3 hash, for every eddy and streak. |
| `naval-ocean/shaders/surface.wgsl`, `foam.wgsl` | `OceanSurfaceMaterial.bind`, `field.surface`, `foam.churnedWater`, `aeration` | The wake height in the vertex stage, `surfaceNormal`, slick-stilled cascades (`slickShares`), churned water, wake bubbles. |
| `naval-water-effects/src/{splash,plumes,particles,bow_spray,strands,textures,lighting,layer}.rs`, `shaders/{spray,sprite,jet}.wgsl` | `CombatEffects.splash`, `WaterPlumes.ts`, `EffectParticles.ts`, `BowSpray.ts`, `BowSprayStrands.ts`, `SprayMaterial.ts`, `EffectLighting.ts` | See "Splashes and bow spray". |
| `naval-water-effects/shaders/caster.wgsl` | none (native only) | A splash's shadow ("Sun shadows"). |
| `naval-water-effects/shaders/cloud.wgsl` | none (native only) | A splash's clouds of spray from the smoke's gas atlas ("Splash clouds and plumes"). |
| `naval-water-effects/src/fixture.rs`, `drive.rs` | the inputs `ShipWake.update` and `CombatEffects` read | The fixture format, `SeaWaves` (naval-sim's `SeaState` heights), the web's `randomFor` stream; frames to `WakeShip`s. |
| `naval-water-effects/src/hulls.rs`, `shaders/hull.wgsl` | none (stand-in) | Proxy hulls and the scene-copy seam (below). |

## Bind groups: the wake joins group 1

The surface already used the four bind groups WebGPU guarantees (0 camera, 1 surface, 2 wave field, 3 sky environment),
so the wakes take no group of their own. They join the surface's group 1 at bindings 5–10:

| Binding | What | Stages |
| --- | --- | --- |
| 5 | `Wake` uniform (39,664 bytes): the field's display origins and weights, trail and torpedo clocks and reach, shading knobs, bow wave slots and knobs, contact foam slots (8 × 263 vec4), 64 trail tiles × 5 rows | vertex, fragment |
| 6, 7 | the wake field's two display textures (`Rgba16Float`), blended by the step fraction without rebinding when they ping-pong | vertex, fragment |
| 8 | linear clamp sampler (wake field, trail atlas, torpedo field) | vertex, fragment |
| 9 | trail atlas (`Rgba8Unorm`, 2,048² on High: churned water in red, slick in green, where each lies across its band in blue and alpha) | fragment |
| 10 | torpedo field (`R8Unorm`, 1,024²) | fragment |

One packed uniform, as the web packs its arrays with `PackedVec4Arrays`, keeps the surface at 5 uniform buffers, 11
sampled textures and 4 samplers per stage, well inside the defaults. `OceanSurface::set_wake` binds a `ShipWake` (or a
calm one: zeroed uniform, black 1 × 1 textures, which reproduces the open sea exactly); `Ocean::enable_wake` makes one for
the tier (field resolution 0/256/512/1,024 for Low/Medium/High/Ultra, as `quality.ts`) and rebinds when the field is
reset. The render core II lane's note that group 0 will carry shadow maps changes nothing here: every pipeline takes
`ctx.camera_layout`.

## Per frame

```rust
ocean.wake.as_mut().unwrap().update(&WakeInputs { ships, dt, splashes, torpedoes, camera, sea_height }); // CPU
effects.update(&EffectsFrame { dt, camera, ships, events, sea, waves, crest, lighting });                  // CPU
renderer.render(frame, &mut [&mut hulls, &mut ocean, &mut sky, &mut effects], output)?;
// Ocean::prepare encodes the wake's GPU half: field steps, atlas repaints, the uniform upload.
```

`naval_water_effects::drive` turns a `FixtureFrame` (what a packet reduces to: hull poses and displayed speeds, running
torpedoes, water events) into these inputs; the vertical slice builds the same from `PacketReader::frame(now)` and
`take_events()`.

## How it matches

Replaying the web's recorded inputs (`compare.json`; sea below the horizon, mean sRGB each side, mean absolute
difference per channel, share of pixels more than 24 levels apart):

| Capture | Web mean | Native mean | Mean abs. diff | > 24 levels |
| --- | --- | --- | --- | --- |
| `wake-air` | 23.1, 39.5, 52.1 | 23.0, 39.3, 51.9 | 2.47 | 2.0% |
| `wake-stern` | 65.9, 94.0, 116.7 | 66.1, 94.9, 117.9 | 5.73 | 7.7% |
| `wake-destroyer` | 53.2, 83.7, 107.4 | 52.0, 83.5, 107.8 | 5.27 | 7.0% |
| `torpedo-track-0` | 33.4, 72.6, 103.2 | 34.0, 73.0, 103.6 | 3.67 | 1.0% |
| `torpedo-track-1` | 35.5, 74.8, 105.7 | 35.9, 75.1, 106.0 | 3.37 | 0.9% |
| `bow-wave-ahead` | 41.0, 80.5, 111.6 | 40.7, 80.8, 112.3 | 5.91 | 5.2% |
| `splash-0…5` | 41.2–45.0 … | 38.1–42.3 … | 6.6–7.3 | 10–12% |
| `bow-wave` (hull fills the frame) | 35.8, 68.4, 94.8 | 34.9, 70.3, 98.9 | 13.9 | 26% |
| `bow-spray-heavy` (hull fills the frame) | 28.2, 46.1, 62.1 | 41.4, 62.0, 81.5 | 17.9 | 31% |

- **The wake field** is the web's to within half a percent: `tests/wake.rs` replays `scripts/diagnostics/ocean-wake.html`
  (a Bismarck's bow and stern generators, 30 s at 15 m/s, a turn, a stop, a teleport, the breaking run) and lands on the
  web's figures from the same machine: trail foam 150 m astern 0.0794 (web 0.0802), peak height 0.30401 m (0.30396),
  total foam 155.98 (155.91), foam widths 24 and 30 m (23.5, 31.5), foam after the stop 17.37 (17.42), breaking foam
  578.5 (578.3). (The web page's own `visibleTrail` check, trail foam > 0.35, fails on master at 0.08: stale.)
- **Trails**: the churned band, its lobes and eddies, the slick and the bubble cloud land on the same pixels; the
  difference images show only pixel-scale edges of the foam pattern.
- **Splashes**: the jets, drops, billows, mist and the trail's foam rings match in shape, size, timing and place, because
  each event draws the web's `randomFor(sequence × 7919 + shell id)` stream.
- **Bow waves**: the stem crest, the heap, the peel-off and the white water along the proxy hull match the web's where
  the hull shape allows.
- **Torpedo tracks** match (thin at the web scene's 200 m; `battle/torpedo-track.png` shows a spread's tracks close up).

What does not match, and why:

- **Hulls are proxies** (`SimpleHull`): box-like grey hulls with a teak deck. The two frames dominated by a hull
  (`bow-wave`, `bow-spray-heavy`) differ mostly in the hull itself. The contact foam and bow spray read the proxy's
  profile, so their line follows a simpler entrance than the Bismarck's or Fletcher's.
- ~~**No sea around hulls**~~: ported (`sea-sky.md`); against the web with it on, the replays now land within the
  same levels as the uncoupled pair (`wake-stern` 4.34, `bow-spray-heavy-ahead` 3.90).
- **Screen-space reflections** of hulls are not ported; the web mirrors the real hull's image in the water beside it.
  The hull shelter (the darker band of water hugging a hull, darkest in her lee) is ported and reads the proxy's side
  heights, so it follows a flat-sided hull.
- ~~**No distance fog on spray**~~: since 2026-10-03 spray, splashes, jets, bow sheets, drops and mist take the aerial
  perspective (`spray_fog` in `shaders/spray.wgsl`, `haze.md`).
- **Funnel smoke, clouds, bloom and FXAA** are other lanes'.
- **Particle cameras during the sail scenes**: the page recorded where its chase camera was, not where it looked; the
  replay aims it at the focus hull. It only sets the bow spray's level of detail while replaying.

## Scene depth (render core II)

The ocean reads the opaque scene behind the water (straight-through transmission, and the thin water column that draws
the shoreline foam line along a hull). Render core II's copy points (`SceneCopies::OPAQUE_COLOR | OPAQUE_DEPTH`,
`Phase::Water`) were published on its branch but are not on master. Until they are, `HullLayer::prepass` draws the proxy
hulls a second time in its `prepare` into colour and depth targets of its own, which the host hands to
`Ocean::set_scene` (which keeps the wake bound). To switch: drop the prepass, move the ocean's draw to `Phase::Water`,
return `OPAQUE_COLOR | OPAQUE_DEPTH` from `Ocean::scene_copies`, and bind `scene.opaque.color`/`scene.opaque.depth` when
their `version` changes. The splashes' and bow spray's sprites fade into the water at each particle's own water level
(the web's) and, since 2026-10-04, into the sea and hulls behind them through `BACKDROP_DEPTH` (soft particles: group 3
of every sprite pipeline, asked for while any sprite lives; a far-plane placeholder until the first copy).

## GPU cost

Apple M5 Pro, Metal, 2026-09-30, load average 85–100 all day (other agents' builds and browsers), High tier:

- **Wake field**: 0.44–0.59 ms per 512² step (reduce and collapse over four levels, the step, the display; batches of 30
  timed to completion, as the web's diagnostic times its steps). The web's own step on the same machine and load: 0.74 ms.
  Steps run at 30 Hz, so about 0.25 ms a 60 Hz frame.
- **Trail atlas repaint**: 0.14–0.27 ms for 500–600 stamps; it repaints at 20 Hz near (5 Hz beyond 2.5 km), under 0.1 ms
  a frame.
- **Surface shading of the wakes**: below the noise of serialised whole frames (2.9–3.0 ms at 1600 × 900 with and
  without the wakes bound, 150 interleaved frames each); the sampler exits early outside the trails' boxes and the bow
  wave and contact foam reaches. Measure again on a quiet machine before budgeting.
- **Splashes and spray**: CPU 0.23 ms for nine heavy splashes at peak, 0.135 ms for four destroyer bows at 28 kn; their
  draws about 1.4 ms at 1280 × 720 at the salvo's peak (serialised frames with and without). Their shadows (2026-10-04)
  add about 0.1 ms to each sun shadow map's pass with eighteen columns standing ("Sun shadows").
- **CPU wake update** (trails, torpedo tracks, bow waves, contact foam, packing): a 941-frame replay of 90 s takes about
  1 s including every frame's GPU submission.

## Splashes and bow spray (naval-water-effects)

Ports `CombatEffects.splash`/`splashBillows` and the water events of `CombatEffects.emit`, `WaterPlumes.ts`,
`BowSpray.ts`, `BowSprayStrands.ts`, `EffectParticles.ts` (the pool), `SprayMaterial.ts` (sprayRadiance, jet, sheet and
sprite materials), `EffectLighting.ts` and the maps (`effectTexture('droplet'|'smoke')`, `smokeBillowTexture`,
`sprayStreakTexture`).

- CPU simulation, GPU drawing, as the web: `ParticlePool` (claim ring giving up the particle nearest its end, analytic
  drag and gravity, waterline kill, back-to-front stable sort, billboard/velocity/streak/water alignment) publishes 64-byte
  `SpriteInstance`s; `WaterPlumes` and `BowSprayStrands` build `JetVertex` strips (spine fixed in the world, width turned
  to the camera).
- `WaterEffects` is one `SceneLayer` in `Phase::Transparent`, depth tested not written, straight-alpha blending (three's
  NormalBlending), linear HDR, pipelines through `RenderContext::add_scene_pipeline` (hot reload): `sprite.wgsl`
  (`fragment_spray` for billows, `fragment_basic` for drops and mist), `jet.wgsl` (`fragment_jet`, `fragment_sheet`),
  `cloud.wgsl` for a splash's billows once the gas atlas is bound ("Splash clouds and plumes"), and the shared `spray_radiance`
  module in `spray.wgsl`. Group 1: the lighting uniform, a map and its sampler. Group 2:
  the sky's environment (`WaterEffects::set_environment`, naval-ocean's `sky_environment` module; a black placeholder
  until the host binds the sky's).
- Draw order follows three's transparent sort for these origin-placed meshes: splash billows, jets, drops, mist; then bow
  billows, bow drops, sheets.
- Randomness: each water event draws from `SeededRandom::for_event(sequence, shell id)`; the bow spray from one LCG
  seeded 1 and its own integer `hash`/`noise`, in the web's call order.
- Lighting: `EffectLighting::from_sky(direction, colour, intensity, night, ambient, in_port)` does what
  `VisualEnvironment.syncLighting` does; the stills take the page's own values. Once the sky's environment is bound,
  the shade's sky light is the environment's from above (`sky_fill`, as the combat effects', so spray dims with dusk,
  night and overcast; `flash` lifts it with lightning), and unlit drops and mist take `scene_light` (the sky's and the
  sun's or moon's light against a clear noon's) instead of the authored `daylight`.
- Bow shapes come from a `WaterlineProfile` (the proxy's until the ships lane slices real models); the stem crest is
  naval-ocean's `bow_waves::stem_crest`.
- Pinned to the web: texels of all four maps, the random stream, `bowShape`/`bowSide` on the proxy profile, jet counts
  (38 heavy, 18 light), pool sizes, `SHIP_PACE` pacing of thrown water.
- **Spray, not cotton** (2026-10-04; `sprite.wgsl` `fragment_spray`): a storm's bow spray drew as white cotton balls,
  each billow the map's one rounded cauliflower. Each sprite now reads the billow map through a warp of its own (0.3 of
  the quad, windowed off its edges), its density raised to 1.7 and torn by value noise of its own at three scales
  (`SpriteInstance::seed`, fixed for the particle's life), thinning further with age (`SpriteInstance::aged`), clumped
  inside (0.45 in the gaps), with 60 % of the map's relief and 80 % of the sky's light in its shade. Every sprite
  (billows, drops, mist) fades over 0.3 of its width (at least 0.2 m) where it meets the sea or a hull behind it.
- **Spindrift** (2026-10-03), the spray a gale tears off the breaking crests, is the ocean's, not this crate's: it comes
  from the wave field's own breaking (`naval-ocean`'s `src/spindrift.rs`; `ocean.md`, "Weather at sea"). Its wisps are
  lit as this crate's white water, with `spray.wgsl`'s constants copied into `spindrift.wgsl` (this crate depends on
  naval-ocean): change them together. Since 2026-10-04 they also take the ships' and columns' shadows on the sea
  (`ocean.md`, "Weather at sea").

## Sun shadows (2026-10-04)

Until then nothing in this crate read the renderer's sun shadow maps: a splash, its jets or a bow's spray standing in a
hull's shadow stayed fully sunlit, and a column standing in the sun threw no shadow. Now:

- **Receiving** (`spray.wgsl` `particle_sun`): every sprite (at its four corners) and every strip (at each vertex)
  reads `naval::shadow::sun_shadow` in the vertex stage, which takes the near, wide and view maps (spray stands beside
  distant hulls too, which only the view maps hold); the fragment stages interpolate it. It scales the sun's or moon's
  beam in `spray_radiance` (diffuse and the glow looking into the sun) and in `scene_light` for unlit drops and mist (the
  authored daylight keeps 0.3 of itself in shadow without the sky's environment). Each point is lifted up the beam before
  the lookup: a sprite by half its width, a jet strip by twice its column's girth (`JetVertex::reach`), a bow sheet by
  0.5 m (`LEAST_REACH`). Moving a point along the light's ray does not move it on the maps, only changes the depth it is
  compared at, so a hull or a column farther up the beam still shades it while the column's own caster leaves its far
  side to the shading the strips are built with (`state.z`). Spray right against a hull's side (within that lift of
  it) can stay lit.
- **Casting** (`shaders/caster.wgsl`, `WaterEffects::draw_shadow`, `cast_shadows`): a splash's jets and its billows
  draw into every map drawn this frame as a screen door, keeping a share of their texels as dense as their water
  (interleaved gradient noise on the map's texels), which the maps' soft filter averages into a partial shadow on the
  sea (foam and glitter show it, as the hulls' own), the hulls and the other spray. Every caster shares the one
  pattern, so where jets and billows overlap a texel is kept by the densest of them, not their sum. The jets' strips are turned to the eye on the CPU; the caster reads
  them from their buffer as storage (`STORAGE` on the jets' batch, group 2) by the index the strips' index buffer gives
  and turns each to the light (spine and width from its two edge vertices, its flight from the rows either side). The
  billows are billboards to the light; faint ones (the veil aloft, the last of a settling cloud: under 0.1 of the sun
  stopped) cast nothing. A column stops at most 0.55 of the sun anywhere (`JET_OPACITY`; billows 0.45), however many
  jets stand in it, and a jet's foot (its first 0.3, down to 0.35 of its density at the water) and a billow's rim
  thin away, so the shadow has no hard edge. Bow spray, drops and mist cast nothing.
- **Why the cap** (2026-10-05): the first version shifted each strip's pattern by its seed, so overlapping jets summed
  like layers of spray; a young column's foot then stopped nearly all the sun and, looking into it, lay over the
  glitter as a near-black slab (0.8–0.98 of the glitter gone, a hull's shadow takes 0.9). With the shared pattern and
  the cap, the same views lose 0.2–0.4 of the glitter (95th percentile; a 1 s column at most about half a hull's), fading
  out softly, and a column two seconds old, torn and falling, barely shades the sea.
- **Not done**: the clouds' shadow on spray (the splashes and bow spray never took it; the spindrift does), and jets
  out of the eye's view do not cast (the CPU culls them before building their strips), so a column just off screen
  throws no shadow into view.

Seen in the client (staged through the debug RPC, world cameras from `effects_probe`: a Bismarck firing on a static Bismarck at 6 km, the maps on the
target, morning sun 28° up, clear): a 380 mm column's shadow runs across its own foam ring and, looking into the sun,
lies as a soft grey lane over the glitter toward the eye; the low billows between a splash and the hull it straddles grey in
her shadow.

Cost (Apple M5 Pro, Metal, 1600 × 900 at the High preset's 70 % scale, load average 30–60 from other sessions'
builds; median of 300 frames' pass timestamps, alternating rounds of master's and this build at one frozen moment
with eighteen columns standing and their billows; again after the cap on 2026-10-05, load average 50–60): the near and
wide maps' passes span 0.05–0.07 ms on master and 0.14–0.24 ms here, about 0.1 ms a map, nearly all the jets (the billows about 0.02 ms). Both passes overlap the
frame's other work (their serial cost stays 0) and whole frames did not change beyond the run's noise. The transparent
pass's serial cost did not move (0.49–0.65 ms either build): the receivers' reads are per vertex.

## At range (2026-10-06)

Theatre gun actions are fought at 20–30 km (the fleet action realism programme), where the fall of shot is spotted
through the binoculars. A heavy splash there is a few pixels across a jet even at 16× (a 9 m jet at 20 km is about 5 px
on a 1030 px frame), and seen up-sun it was no brighter than the hazed horizon behind it: the jets' tears and gaps,
the billows' clumps and the column's shaded side averaged to the sky's own level (the brightest 0.5 % of a box about a
Hipper 20 km off, 4 s after a Bismarck salvo, 0.5 % over the sky above at 16×), and the salvo vanished. Down-sun the
same column stood out (+12 %).

`far_share` (`shaders/spray.wgsl`) reads how many pixels a jet or a billow is drawn across from its own `u`
(`fwidth`): from 16 px down to 3 px it is drawn whole. The jet's fill closes into a solid column thinning toward its
tip and its tears take the streaks' mean (so it still thins as it ages); the billow keeps its mass without its tears
and clumps; and the shaded side takes `FAR_SCATTER` (0.5) of the direct beam, the light many drops deep scatter round
to it, as a cumulus's shaded side keeps about half its sunlit side's (`spray_radiance`'s `far`). A resolved column (a
splash a few hundred metres off, or a far one at high magnification once it fills the glasses) draws as before; the
bow's sheets keep their detail at every range.

Measured with `bun native/tools/long-range.ts` (`harness.md`, "Long-range action"; noon, a Bismarck firing on a
Hipper, the brightest 0.5 % of a box about the target 4 s after the first fall against the sky above it; stills in
`.build/native/long-range/{before,after}-*`, master 78c82552a against this branch):

| Air, bearing | Range, lens | Master | Here |
| --- | --- | --- | --- |
| Clear (35 km), up-sun | 20 km, 16× | +0.5 % | +6.0 % |
| Clear, up-sun | 20 km, 8× | +0.6 % | +4.7 % |
| Clear, up-sun | 25 km, 16× | +0.2 % | +4.3 % |
| Clear, up-sun | 25 km, 8× | +0.6 % | +2.8 % |
| Clear, down-sun | 20 km, 16× | +11.6 % | +11.6 % |
| Clear, down-sun | 25 km, 16× | +8.8 % | +8.6 % |
| North Atlantic's own (25 km), up-sun | 20 km, 16× | +1.3 % | +5.1 % |
| North Atlantic's own, up-sun | 25 km, 16× | +0.7 % | +3.3 % |
| 18 km visibility, up-sun | 20 km, 16× | +0.6 % | +2.1 % |

Down-sun nothing changed (the sunlit face was already brighter than the floor); up-sun the columns read as pale white
water where they were a blue veil. In 18 km of visibility a column 25 km off is gone either way, as the haze means
(`haze.md`: a ship goes at about the visibility, and so does her splash). Whole frames from those views took the same
time on both builds (medians 8.0–8.6 ms on master, 7.9–8.3 ms here, 1728 × 1030 offscreen, M5 Pro, load average 70–85):
the change is a few ALU instructions a fragment. Columns 1.5–6 km off at 5–20° lenses look as before
(`.build/native/long-range/mid/columns-pair.png`).

## Splash clouds and plumes (2026-10-08)

A shell's splash read as a fan of flat white blades: 38 camera-facing jets, evenly white and translucent with fine
streaks, a few flat billows of the 2D billow map at the foot, and on the sea a foam ring and boil torn along one level
of the eddies, which after 5–10 s was a marbled donut with a hole in the middle and a cut outline. The fan stays (the
owner's pick); now:

- **Each jet is a plume of white water** (`shaders/jet.wgsl` `fragment_jet`): billowing lumps fixed to its water
  (four octaves of the magnitude of gradient noise, rounded lumps parted by sharp creases, measured in the jet's own
  widths along it from its length at the top of its flight, `JetVertex::stretch`, drawn out a little along its flight)
  push its outline out (0.75 of a half-width) and tilt its lit faces off the rounded column (the finer octaves barely:
  their creases lit as shards). A lump's crease keeps 0.55 of the sky and 0.6 of the sun, so a side- or back-lit jet
  shows grey folds; front-lit it stays white with soft relief. Feathers of torn spray streak past its sides along its
  flight, the strip drawn 1.35 times its water's width for them (`FRINGE`, also in `caster.wgsl`, whose shadow stays the
  water's), and it tears between its lumps as it ages. Seen a few pixels across it is drawn whole as before
  (`far_share`).
- **The clouds of spray are the smoke's puffs** (`shaders/cloud.wgsl`, `Material::Cloud`): a splash's billows are drawn
  from the smoke's gas atlas (naval-effects' sixteen puffs baked six ways, `EffectsLayer::gas_atlas`, bound by the
  client with `WaterEffects::set_gas_atlas`), weighted by the sun in each puff's frame as `gas_sprite.wgsl` does, and
  relit as white water: 0.42 of the bake's optical depth, the shade lifted toward the lit side (0.55, light many drops
  deep goes round), the thin rim glowing looking toward the sun, 1.9 × the sky's light. As a puff ages its rim tears
  away first and it thins (the gas's dispersal). It fades into the sea over its own size (at least 4 m), not at a level
  line. With the atlas unbound (examples, tests) the billows fall back to `sprite.wgsl`'s `fragment_spray`.
- **The mist the jets shed** (`splash.rs`, `splash_billows`): puffs (heavy 14, light 6) gather 0.25–0.6 s after the
  jets lead, rise with them and stall under drag at their own share of the lower 0.7 of the column (60 m × size ×
  lift), swell, then settle at about 1.5 m/s (life 4.5–10.5 s), thinner than the jets (0.55 opaque, less higher up) so
  the fan reads through them. The base's dense spray, the low wall, the veil and the dome are as before, shorter-lived
  (5–7, 7–10, 9–13, 6–8 s). A heavy splash throws 40 billows (24 before); the pool holds 4,096.
- **The foam is a patch** (naval-ocean `trail.rs`, `wake.wgsl`): a splash stamps one soft disc that spreads to about
  33 m × scale in 6 s and keeps growing slowly, its strength falling from about 0.9 to 0.45 in the first seconds and out
  between 6 s and 30 s (`IMPACT_LIFETIME`), and a brighter front ring for the first 10 s (the base surge). The shader
  keeps a splash's energy smooth, mottled broadly over 2.8 eddy lengths (never below 0.3 of its cover), and lets the
  surface's foam texture tear it into lace (`churned_water`), so the patch thins as it fades instead of being cut along
  the eddies. Dense white water only under the column's first second or two (`SPLASH_YOUNG`).

Seen with `bun native/tools/effects-trial.ts --only splash,splash-near,splash-above,splash-far --stills` (scenes added
for this: a 38 cm near miss by a player salvo in `DUEL`, seen low at 260 m, near at 130 m, from 140 m up and from
1.2 km; `--hours 9 --size 1920x1080` puts the sun behind the near view's column), master's build with `--binary`
against this one. GPU busy at 1280 × 720 (`--cost 30`, M5 Pro, load average 25–35): the transparent pass 1.0–1.15 ms
against 0.92–1.03 ms on master with the salvo's columns close up, whole frames 4.45–4.8 ms against 4.34–4.6 ms. Through
the binoculars at 20 km up-sun (`long-range.ts --ranges 20 --mags 16 --bearing 180`) the fall of shot reads as before.

## The blast on the sea under the guns (2026-10-08)

A heavy turret's blast laid a dark disc on the sea around its smoke, wider than the smoke and from the instant it
fired: the smoke's shadow (`combat-effects.md`, "Sun shadows"), its balls drawn at the full-grown puff's size while
the flipbook draws a young blast's gas at about half of it, and a burning fireball casting as smoke. That is fixed in
naval-effects (the balls are the gas as drawn, and a fireball casts once it cools). What the sea under the guns of a
battleship really shows is a sheet of white spray: the photographs of an Iowa's broadside (Vieques, 1984) show a flat
white disc reaching 50–60 m out from her side under each turret, a sharp bright rim, a thinner middle the sea shows
through, lit orange by the fireball over it.

**The sheet** (`src/blast_sheet.rs`, `shaders/blast.wgsl`). The blast's wind tears a layer of fine spray off the
waves; the water barely moves (the pressure pulse lasts hundredths of a second and presses the sea down by millimetres),
so nothing is laid on the wave field. Where the sheet reaches is where the blast is still strong enough at the sea: its
overpressure falling as r^−1.6 and leaning along the bore as (½ + ½ cos φ)^1.3 (at right angles 40 % of the overpressure
along it, behind the muzzle a tenth, never less: `FLOOR`), its reach straight ahead 95 calibres for one barrel, growing
as the cube root of the barrels firing together (Hopkinson–Cranz scaling): an Iowa's triple 16-inch turret reaches 56 m.
The sheet is the sea within that reach of the muzzles, an egg under and ahead of them, smaller for guns standing higher
or raised to a long range, and nothing for a 5-inch mount 20 m up. Its density comes in between 10 and 24 cm calibre.
The shock sweeps it out at 340 m/s (the reach in a sixth of a second), the spray coasts 22 % past the reach over half a
second, piled into a bright rim; behind the rim it clears in about 0.6 s, the rim in 1.3 s, both torn into streaks
along the blast and clumps by noise of the sheet's own, and it drifts with the rest of the spray (the lighting's wind)
until it is gone at 3.4 s.

**Drawing it.** One horizontal quad a sheet over its footprint, at the top of its mist (2 m over the water and half the
significant wave height more), grown by the run of a grazing view's column (top ÷ 0.12). Each fragment follows its view
ray down to what the backdrop's depth puts behind it (the sea, or a hull's side) and adds up the mist on the way: two
layers whose density falls with height over the sea (the middle's over 0.8 m, the rim's piled spray over 1.6 m), read
where the ray is low in it, so a sheet seen from a deck stands out as a white band and seen from overhead is a veil,
and a hull's side or a wave crest standing up through it cuts its column short. It is lit as spray (`spray_radiance`,
flat, with the sun's shadow maps and the smoke's soft occluders at the foot of its column) and by the scene's point
lights as the sea takes them (`WaterEffects::point_lights`, the host's `sea_lights`): the turret's flash, and the
fireball's glow, which the flash light now keeps for about a second (naval-effects' `FlashRequest::glow`). It is drawn
first in the transparent phase, under every splash and under the combat smoke.

**Feeding it.** naval-effects groups a salvo's barrels into turret blasts as before and keeps each turret's
(`CombatEffects::turret_blasts`: the muzzles' centre, the bore, the calibre, the barrels and the sequence); the client
hands them to `WaterEffects::gun_blasts` each frame (`naval-client/src/effects.rs`) and the sheet's water level is the
combat sea's at the muzzles' foot. `EffectCounts::blasts` counts the sheets on the sea.

Seen with `bun native/tools/effects-trial.ts --only muzzle-above,muzzle-side,muzzle-low --stills` (an Iowa's first
salvo at a still Bismarck 6 km off: from 300 m over her engaged side, from 400 m off it, forward and 80 m up, as the
photographs were taken, and from 320 m off it 12 m up, as from another ship's bridge, where the sheets are bands of
white along her waterline); `--hours 23` for the night, where the sheets take the moon's and the flashes' light.
GPU busy at 1280 × 720 (`--cost 30`, M5 Pro, two interleaved rounds against this branch's parent, load average 25–37
from other agents' builds): the transparent pass a median 0.15 ms more over the six moments the sheets lie on the sea
seen from 300 m overhead (where they fill much of the frame) and 0.09 ms from the side, against 0.03 ms in the moments
after they are gone; single moments ran from −0.03 to +0.71 ms, the noise of this load.

**Open.** One water level a sheet, the combat sea's at the muzzles' foot. The sheet leaves no foam on the sea, and a
camera inside its mist below the quad sees none of it. It does not shade what is under it.

## Tests

- `naval-ocean`: `wake::kernel` (the pyramid sums to zero, follows |k| within 4% from 6 to 12 × 2^levels cells, stays
  positive, matches a CPU reduce/collapse), `wake::generators`, `trail`, `torpedo_tracks`, `bow_waves`, `hull_profile`,
  every shader composes and validates, and `tests/wake.rs` (the GPU field against the web's diagnostic figures).
- `naval-water-effects`: fixture round trip, the random stream, proxy hull meshes, the particle ports (texels, counts,
  integration, bow shapes), the blast sheets (a broadside's footprint, higher or raised guns laying less and a light gun
  none, the reach along and off the bore, sheets laid, drifting and gone), a GPU smoke test, and `tests/fixture_playback.rs`: 100 s of a native battle recorded through
  `naval-sim-host` drives the trails, tracks, bow waves, contact foam, splashes and spray twice with identical results
  (needs `bun run sim:content`, as naval-sim-host's tests do).

## Open problems and follow-ups for the vertical slice

Done by the integration lane (batch 3): the slice drives the wakes, splashes and bow spray live from the packets and
`take_events()` (`naval-client/src/effects.rs`), the ocean reads render core II's opaque copies (no hull prepass),
and each preset's `WaterlineProfile` is sliced from its real model (`naval_ocean::hull_slice::profile_from_triangles`,
a port of `hullWaterlineProfile.ts`). Still open: screen-space reflections, fog on spray, the
lens's own hull, and the effects density setting.

- **Render core II**: switch the ocean to `Phase::Water` and the opaque copies; drop the hull prepass.
- **Real hulls**: slice the ships lane's meshes into `WaterlineProfile` (port `hullWaterlineProfile`), and take each
  hull's `wakeHull` extents from its model.
- ~~**Sea around hulls**~~: done (`sea-sky.md`).
- **Screen-space reflections** in the surface; **fog** on spray (a fog module the effects can import without the sky's
  bindings).
- **Live driving**: build `WakeInputs` and `EffectsFrame` from `PacketReader::frame(now)` (interpolated poses,
  `ShipVisual::speed`, torpedoes) and `take_events()`; hide the lens's own hull (`EffectShip::visible`).
- The camera the wakes read for the trail refresh rate and torpedo anchoring wants the real forward vector and
  `projection[5]`; `WakeCamera` takes both.
- `ShipWake`'s and `WaterEffects`' pipelines are built with the device, not the renderer's registry, except the particle
  and hull pipelines: no hot reload for the wake shaders yet (as the ocean's own).
- The combat effects density setting is not wired to a graphics setting.

## Re-sync

Web sources re-read against master `6cd598cf2` (2026-09-30) by the re-sync lane (`docs/archive/native-resync.md`); the next re-sync diffs
from there. `BowSpray.ts` flies its thrown water at the battle's ship pace: ported as `BowSprayFrame::pace` (kept per bow,
as the web does) and `WaterEffects::ship_pace`, which `naval-client` sets from `Battle::pace` (the default 1.5 in every
battle but a theatre bubble's, so no picture changed; `thrown_water_flies_at_the_battles_pace` checks the scaling).
`ShipWake.ts`' new `layTrails` (a theatre scene's ships come aboard with their trails laid) is left for the
theatre-native lane; `FleetWakeFoam.ts` changed a comment. `splash.rs`'s sources did not change.

## Needs from naval-sim

Nothing blocks this lane. For the sea around hulls, the combat sea's components (`SeaState` is public; the fixture
records it from `Battle::sea`). A packet field carrying it at start and on a wind change would save the host reading the
battle directly.
