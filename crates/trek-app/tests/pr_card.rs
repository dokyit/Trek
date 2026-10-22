//! The PR/release card's command tracking and row ids (`src/pr_card.rs`).

use trek_app::pr_card::{self, Call, Card, Kind, Phase};

#[test]
fn finds_the_calls_it_cards() {
    let d = "gh pr create --title \"Add the thing\" --base main --head feat/x";
    assert_eq!(
        pr_card::call("Run command", d),
        Some(Call::PrCreate {
            title: Some("Add the thing".into()),
            base: Some("main".into()),
            head: Some("feat/x".into()),
        })
    );
    assert_eq!(
        pr_card::call("Run command", "gh pr merge 128 --merge --delete-branch"),
        Some(Call::PrMerge(Some("128".into())))
    );
    assert_eq!(
        pr_card::call(
            "Run command",
            "gh pr merge https://github.com/o/r/pull/9 --squash"
        ),
        Some(Call::PrMerge(Some("https://github.com/o/r/pull/9".into())))
    );
    assert_eq!(
        pr_card::call("Run command", "gh run watch"),
        Some(Call::Checks)
    );
    assert_eq!(
        pr_card::call("Run command", "gh pr checks --watch"),
        Some(Call::Checks)
    );
    assert_eq!(
        pr_card::call("Run command", "script/release.sh 0.4.1 --channel beta"),
        Some(Call::Release {
            version: "0.4.1".into(),
            channel: "beta".into()
        })
    );
    assert_eq!(
        pr_card::call(
            "Run command",
            "gh workflow run release.yml -f version=0.4.1 -f channel=beta"
        ),
        Some(Call::ReleaseWorkflow {
            version: "0.4.1".into(),
            channel: "beta".into()
        })
    );
    assert_eq!(
        pr_card::call("Run command", "gh release create v0.4.1"),
        Some(Call::ReleaseTag {
            tag: "v0.4.1".into()
        })
    );
    // Other gh calls, and calls that aren't commands, make no card.
    assert_eq!(pr_card::call("Run command", "gh pr view 128"), None);
    assert_eq!(pr_card::call("Read", "gh pr create"), None);
    assert_eq!(
        pr_card::call(
            "Run command",
            "cargo build && gh pr create --title t --base main"
        ),
        Some(Call::PrCreate {
            title: Some("t".into()),
            base: Some("main".into()),
            head: None,
        })
    );
}

#[test]
fn pr_urls_come_out_of_output() {
    assert_eq!(
        pr_card::pr_url("https://github.com/o/r/pull/128"),
        Some(("https://github.com/o/r/pull/128".into(), 128, "o/r".into()))
    );
    assert_eq!(
        pr_card::pr_url("remote: Create a pull request\nhttps://github.com/o/r/pull/9\n"),
        Some(("https://github.com/o/r/pull/9".into(), 9, "o/r".into()))
    );
    assert_eq!(pr_card::pr_url("no url here"), None);
}

#[test]
fn rows_roundtrip() {
    let card = Card {
        kind: Kind::Pr,
        number: Some(128),
        phase: Phase::Checking,
        ..Default::default()
    };
    let detail = serde_json::to_string(&card).unwrap();
    assert_eq!(pr_card::of_row("trek-task:x"), None);
    let c = pr_card::read(&pr_card::row_id("k"), &detail).unwrap();
    assert_eq!(c.number, Some(128));
    assert_eq!(c.phase, Phase::Checking);
    assert_eq!(c.chip(), "#128 Checking");
}
