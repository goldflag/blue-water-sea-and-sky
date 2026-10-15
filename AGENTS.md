# For agents reading this repository

This repository is reference source, not a buildable project. Do not try to `cargo build` it: the crates depend on
the game's renderer core, simulation and protocol, which are only partly included (`shared/`).

To write your own ocean, sky or clouds from it:

1. Read [README.md](README.md) for the frame's phases, the couplings between the three systems, and the table mapping
   path names in the sources onto this repository.
2. Read the system's `OVERVIEW.md` (`ocean/`, `sky/`, `clouds/`). Each has a reading order, the algorithms with their
   constants, and the pitfalls the game ran into.
3. Open the files in that reading order. Shaders are in `<system>/shaders/` (WGSL; `#import a::b` is naga_oil's
   module import, and `#define_import_path` names a module); the host code is in `<system>/src/`.
4. For the reason behind a constant or a design choice, search the system's development log in `docs/`
   (`ocean.md`, `sky.md`, `clouds.md`, plus `sea-sky.md`, `haze.md`, `lighting.md`, `water-effects.md`).
5. The tests in `<system>/tests/` state the properties the implementation must keep, with thresholds. They are a
   useful checklist for a new implementation.

Keep the third-party notices (`THIRD_PARTY.md` and the per-directory ones) with any code you port from `sky/` or `clouds/`.
