//! The tree against its parity records (see the crate's docs). With `TREK_PARITY_UPDATE=1` the
//! records are written from the tree instead: the gate counts and the ledger's table.

use std::path::PathBuf;
use trek_parity::{
    Ledger, Recorded, census, gate_changes, ledger_problems, render_gates, render_table,
};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn updating() -> bool {
    std::env::var_os("TREK_PARITY_UPDATE").is_some_and(|v| !v.is_empty() && v != "0")
}

const UPDATE: &str = "TREK_PARITY_UPDATE=1 cargo test -p trek-parity";

#[test]
fn every_platform_gate_is_recorded() {
    let root = root();
    let found = census(&root);
    let path = root.join("docs/parity/gates.toml");
    if updating() {
        std::fs::write(&path, render_gates(&found)).expect("write docs/parity/gates.toml");
        return;
    }
    let recorded: Recorded = toml::from_str(&std::fs::read_to_string(&path).unwrap_or_default())
        .expect("docs/parity/gates.toml reads");
    let changes = gate_changes(&recorded.gates, &found);
    assert!(
        changes.is_empty(),
        "Platform gates moved since they were last recorded:\n\n{}\n\nOnce docs/parity/ledger.toml says what each platform does, record the counts: {UPDATE}",
        changes.join("\n")
    );
}

#[test]
fn the_ledger_is_complete_and_its_table_is_current() {
    let root = root();
    let text = std::fs::read_to_string(root.join("docs/parity/ledger.toml"))
        .expect("docs/parity/ledger.toml");
    let ledger: Ledger = toml::from_str(&text).expect("docs/parity/ledger.toml reads");
    let problems = ledger_problems(&ledger, &root);
    assert!(
        problems.is_empty(),
        "docs/parity/ledger.toml:\n{}",
        problems.join("\n")
    );
    let table = render_table(&ledger);
    let path = root.join("docs/PARITY.md");
    if updating() {
        std::fs::write(&path, &table).expect("write docs/PARITY.md");
        return;
    }
    // A Windows checkout may have turned its line ends into CRLF.
    let current = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        current == table,
        "docs/PARITY.md is older than docs/parity/ledger.toml: {UPDATE}"
    );
}
