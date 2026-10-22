//! Why this file exists: `cargo test` builds a package's binaries as binaries only when one of its
//! integration tests names them (`CARGO_BIN_EXE_*`). Without that, `cargo test --workspace` leaves
//! `target/<profile>/fake-acp` unbuilt, and the first test to call `trek_test_fixtures::bin` runs
//! `cargo build` in the middle of the run. Cold, that took 20 s on a macOS runner and starved the
//! tests that were already waiting on the fake agent (see `bin`).

use std::path::Path;

#[test]
fn the_binaries_are_built_before_any_test_runs_and_found_where_bin_looks() {
    for (name, built) in [
        ("fixture", env!("CARGO_BIN_EXE_fixture")),
        ("fake-acp", env!("CARGO_BIN_EXE_fake-acp")),
        ("fake-mcp", env!("CARGO_BIN_EXE_fake-mcp")),
    ] {
        let found = trek_test_fixtures::bin(name);
        assert!(found.is_file(), "{name}");
        let same = |a: &Path, b: &Path| {
            std::fs::canonicalize(a).unwrap() == std::fs::canonicalize(b).unwrap()
        };
        assert!(
            same(&found, Path::new(built)),
            "bin({name}) is {}, cargo built {built}",
            found.display()
        );
    }
}
