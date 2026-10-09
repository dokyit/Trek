//! Create → turn off → turn on, against a throwaway home folder.
use trek_core::skills::{self, SkillHome, SkillSource};

#[test]
fn skill_lifecycle() {
    let home = std::env::temp_dir().join(format!("trek-home-{}", std::process::id()));
    std::fs::create_dir_all(&home).unwrap();
    // Only this test runs in this binary, so swapping HOME is safe.
    unsafe { std::env::set_var("HOME", &home) };
    let md = skills::create("Trail Notes", "Write trail notes.", SkillHome::Codex).unwrap();
    assert!(md.ends_with(".codex/skills/trail-notes/SKILL.md"));
    let found = skills::discover(None);
    let s = found.iter().find(|s| s.name == "trail-notes").expect("discovered");
    assert_eq!((s.source.clone(), s.enabled), (SkillSource::Codex, true));
    let moved = skills::set_enabled(s, false).unwrap();
    assert!(!home.join(".codex/skills/trail-notes").exists() && moved.exists());
    let off = skills::discover(None).into_iter().find(|s| s.name == "trail-notes").unwrap();
    assert!(!off.enabled);
    assert_eq!(off.source, SkillSource::Codex);
    skills::set_enabled(&off, true).unwrap();
    let back = skills::discover(None).into_iter().find(|s| s.name == "trail-notes").unwrap();
    assert!(back.enabled && home.join(".codex/skills/trail-notes/SKILL.md").exists());
    assert!(!back.dir.join(".trek-origin").exists());
    // (Trash isn't exercised here: it would land in the real user's Trash.)
    let _ = std::fs::remove_dir_all(&home);
}
