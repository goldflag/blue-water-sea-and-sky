# Blue Water: sea and sky

The real-time ocean, physically based sky and volumetric clouds from **Blue Water**, a naval game with a native Rust
client (wgpu, WGSL shaders composed with [naga_oil](https://github.com/bevyengine/naga_oil)).

**This is a reference, not a library.** The code is copied verbatim from the game at one commit and will not build on
its own: it depends on the game's renderer core, simulation and protocol crates, most of which are not included.
It is published so that people, and especially AI coding agents, can read a complete, production-tested
implementation and write their own for their engine, whether that is wgpu, Vulkan, Unity, Unreal, Godot or three.js.

## What's here

| Directory | System | Size | Start with |
| --- | --- | --- | --- |
| [`ocean/`](ocean/) | FFT ocean: JONSWAP spectrum, GPU FFT cascades, clipmap surface, Fresnel/sky/screen-space reflection, sun glitter as glints, whitecap foam, ship wakes and Kelvin bow waves, coasts and surf, lakes, the underwater view | ~29k lines | [ocean/OVERVIEW.md](ocean/OVERVIEW.md) |
| [`sky/`](sky/) | Bruneton precomputed atmospheric scattering (via takram's port), sun, moon and phases, Yale catalogue stars, the Milky Way, the environment probe ships and sea are lit by, aerial perspective | ~12k lines | [sky/OVERVIEW.md](sky/OVERVIEW.md) |
| [`clouds/`](clouds/) | Volumetric clouds (a port of takram's three-clouds): ray march, Beer shadow maps in cascades, temporal upscaling, haze and light shafts, weather, rain and lightning | ~14k lines | [clouds/OVERVIEW.md](clouds/OVERVIEW.md) |
| [`shared/render/`](shared/render/) | The pieces of the game's renderer core the three systems plug into: the `SceneLayer` trait and frame phases (`src/frame.rs`), the shader-module registry (`src/shaders.rs`), aerial perspective, and the WGSL modules they `#import` (`naval::camera`, `naval::aerial`, `naval::shadow`, ...) | | `src/frame.rs` (`SceneLayer`, `Phase`) |
| [`shared/sim-clouds/`](shared/sim-clouds/) | The renderer-free cloud layers, genera and weather presets the clouds read from the simulation | | `layers.rs`, `weathers.rs` |
| [`shared/sim/sun.rs`](shared/sim/sun.rs) | Sun and moon positions and sidereal time for a date, time and place | | |
| [`docs/`](docs/) | The development logs for each system: decisions, measurements, before/after numbers, what looked wrong and why | | the OVERVIEWs first |

Each crate keeps its `tests/` and `examples/`. The GPU tests are worth reading as specifications: they state what
must hold (energy balance, tiling, wake shape, no grain, the sky's light balance) with the numbers the game was held to.

## How the pieces fit in a frame

Every system is a `SceneLayer` (`shared/render/src/frame.rs`) with `prepare` (compute work before the scene pass)
and `draw` per phase. The scene pass draws in four phases, with copies of the HDR colour and depth between them:

1. **Opaque**: hulls, terrain. Then the *opaque copy*: the clouds march here, over the opaque depth.
2. **Water**: the sea, which samples the opaque copy for refraction, the shallows and screen-space reflection.
3. **Sky**: the dome at the far plane where nothing was drawn, then the clouds' composite.
4. **Transparent**: spray, smoke, rain. Then the *backdrop copy*, and the rain's veil.

Couplings between the systems:

- The **sky** bakes an environment probe (a PMREM/CubeUV atlas) that the **ocean** reflects and ships are lit by; the
  clouds are drawn into that bake.
- The **clouds** write a ground-shadow map that the **ocean** multiplies into the sun (`naval_clouds::shadow`).
- The **clouds** and **sky** read the sky's precomputed transmittance and scattering tables; all three apply the same
  aerial perspective (`naval::aerial`).
- The **ocean** writes an underwater probe (each pixel's depth under the waves at the near plane) that the clouds
  and the display read so that nothing is drawn above the sea when the camera is below it.

`clouds/src/game_sky.rs` is where the sky, the clouds, the weather and the rain are composed into one layer. It is the
best single file for seeing the order of the passes.

## Path names in the sources

The sources and docs use the game repository's paths. Here is how they map onto this repository:

| In the sources | Here |
| --- | --- |
| `native/crates/naval-ocean` | `ocean/` |
| `native/crates/naval-sky` | `sky/` |
| `native/crates/naval-clouds` | `clouds/` |
| `native/crates/naval-render` | `shared/render/` (partial) |
| `crates/naval-sim/src/clouds` | `shared/sim-clouds/` |
| `crates/naval-sim/src/theatre/sun.rs` | `shared/sim/sun.rs` |
| `native/docs/*.md` | `docs/` |

Everything else they mention is not included: `src/game/*` (the earlier browser game, which no longer exists, whose
TypeScript the native code was ported from), `naval-protocol`, `naval-client`, the harness tools and the rest of the
renderer. The docs also cite pull-request numbers and capture names from the game's repository; treat them as history.

## Using this as a reference

- Read the system's `OVERVIEW.md`, then the files in its reading order. Use the doc in `docs/` when you need to know
  *why* something is the way it is: most odd-looking constants have a dated entry explaining the artefact they fixed.
- Port ideas, not the plumbing. The bind-group layouts, `naga_oil` composition and `SceneLayer` wiring are specific to
  this renderer; the spectra, the march, the tables and the shading are not.
- The dev logs record failures as well as successes. When a log says an approach was tried and removed, believe it
  and save yourself the trial.

## Licence

MIT, see [LICENSE](LICENSE). The atmosphere and clouds are ports of MIT- and BSD-licensed work, whose notices are in
[THIRD_PARTY.md](THIRD_PARTY.md) and next to the files they cover.

Snapshot of the game at commit `c01739b8c` (2026-10-14).
