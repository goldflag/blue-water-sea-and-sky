//! Every integration test of this crate in one test program: each file in this directory is a module of it, so
//! Cargo links the crate and its dependencies once rather than once per file. `bun run rust:test -- <file stem>`
//! still runs one file (its module); `cargo test --test all -- <stem>::` does the same by hand.

mod detail_noise;
mod gpu;
/// With `autotests = false`, a file in tests/ that is neither declared above nor its own `[[test]]` in Cargo.toml would
/// never be compiled or run.
#[test]
fn every_file_in_tests_is_a_module_here() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = root.join("tests");
    let all = std::fs::read_to_string(dir.join("all.rs")).unwrap();
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if let Some(stem) = name.strip_suffix(".rs").filter(|stem| *stem != "all") {
            let module = [format!("mod {stem};"), format!("mod r#{stem};")]
                .iter()
                .any(|line| all.contains(line.as_str()));
            let own = manifest.contains(&format!("path = \"tests/{name}\""));
            assert!(
                module != own,
                "tests/{name} must be a module of tests/all.rs or its own [[test]] in Cargo.toml, not {}",
                if own { "both" } else { "neither" }
            );
        }
    }
}
