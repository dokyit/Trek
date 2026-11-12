use super::*;

fn rule(kind: RuleKind, pattern: &str, action: RuleAction) -> PermissionRule {
    PermissionRule::new(kind, pattern, action)
}

fn deny(pattern: &str) -> PermissionRule {
    rule(RuleKind::Command, pattern, RuleAction::Deny)
}

fn allow(pattern: &str) -> PermissionRule {
    rule(RuleKind::Command, pattern, RuleAction::Allow)
}

fn action_for(rules: &[PermissionRule], line: &str) -> Option<RuleAction> {
    decide(rules, &Call::command("bash", line), None).map(|v| v.action)
}

#[track_caller]
fn denied(pattern: &str, line: &str) {
    assert_eq!(
        action_for(&[deny(pattern)], line),
        Some(RuleAction::Deny),
        "`{line}` must not get past a deny on `{pattern}`"
    );
}

#[track_caller]
fn passes(pattern: &str, line: &str) {
    assert_eq!(
        action_for(&[deny(pattern)], line),
        None,
        "`{line}` has nothing to do with `{pattern}`"
    );
}

#[track_caller]
fn allowed(pattern: &str, line: &str) {
    assert_eq!(
        action_for(&[allow(pattern)], line),
        Some(RuleAction::Allow),
        "`{line}` is what `{pattern}` allows"
    );
}

#[track_caller]
fn not_allowed(pattern: &str, line: &str) {
    assert_eq!(
        action_for(&[allow(pattern)], line),
        None,
        "`{line}` must not pass as `{pattern}`"
    );
}

#[test]
fn a_deny_sees_a_command_the_way_a_shell_would() {
    for line in [
        "rm x",
        "rm -rf x; echo y",
        "echo y; rm -rf x",
        "echo y && rm x",
        "false || rm x",
        "echo y | rm x",
        "sleep 1 & rm x",
        "echo y\nrm x",
        "echo y\r\nrm x",
        "FOO=1 rm x",
        "FOO=1 BAR=2 rm x",
        "\\rm x",
        "r\\m x",
        "'rm' x",
        "\"rm\" x",
        "r''m x",
        "r\"\"m x",
        "/bin/rm x",
        "/usr/bin/env rm x",
        "./rm x",
        "../../bin/rm x",
        "~/bin/rm x",
        "RM x",
        "/BIN/RM x",
        "rm.exe x",
        "C:\\tools\\rm.exe x",
        "echo $(rm x)",
        "echo `rm x`",
        "echo \"$(rm x)\"",
        "echo \"`rm x`\"",
        "FOO=$(rm x)",
        "FOO=`rm x` ls",
        "echo $(echo $(rm x))",
        "echo ${X:-$(rm x)}",
        "echo $((1 + $(rm x)))",
        "cat <(rm x)",
        "echo y > >(rm x)",
        "(rm x)",
        "{ rm x; }",
        "if true; then rm x; fi",
        "for i in 1 2; do rm $i; done",
        "while true; do rm x; done",
        "f() { rm x; }; f",
        "function f { rm x; }",
        "case y in y) rm x;; esac",
        "! rm x",
        "time rm x",
        "sh -c 'rm x'",
        "bash -c \"rm x\"",
        "/bin/zsh -lc 'rm x'",
        "bash -lc 'sh -c \"rm x\"'",
        "eval rm x",
        "eval 'rm x'",
        "sudo rm x",
        "sudo -u root rm x",
        "env rm x",
        "env -i FOO=1 rm x",
        "command rm x",
        "builtin rm x",
        "exec rm x",
        "nohup rm x",
        "nice -n 5 rm x",
        "timeout 5 rm x",
        "xargs rm",
        "ls | xargs -n1 rm",
        "sudo sh -c 'rm x'",
        "timeout 5 bash -c 'rm x'",
        "watch 'rm x'",
        "ssh host 'rm x'",
        "ssh host rm x",
        "busybox rm x",
        "find . -exec rm {} ;",
        "find . -exec rm {} \\;",
        "find . -execdir rm {} +",
        "fd -x rm",
        "python3 -c 'import os; os.system(\"rm x\")'",
        "python -c \"import os; os.system('rm x')\"",
        "node -e 'require(\"child_process\").execSync(\"rm x\")'",
        "perl -e 'system(\"rm\", \"x\")'",
        "awk 'BEGIN { system(\"rm x\") }'",
        "git -c alias.x='!rm y' x",
        "alias x='rm y'; x",
        "trap 'rm x' EXIT",
        "sh <<EOF\nrm x\nEOF",
        "sh <<'EOF'\nrm x\nEOF",
        "bash <<< 'rm x'",
        "python3 - <<'EOF'\nimport os; os.system(\"rm x\")\nEOF",
        "cmd /c rm x",
        "cmd.exe /C \"rm x\"",
        "powershell -Command rm x",
        "pwsh -c \"rm x\"",
    ] {
        denied("rm", line);
    }
}

#[test]
fn what_cant_be_known_counts_as_a_match_for_a_deny() {
    for line in [
        // The program itself isn't known until it runs.
        "$X y",
        "${X} y",
        "\"$X\" y",
        "$(echo rm) y",
        "`echo rm` y",
        "$'\\x72\\x6d' y",
        "$EMPTY rm y",
        "/bin/r? y",
        "/bin/r* y",
        "r[m] y",
        "{rm,y}",
        // Its code comes from somewhere Trek can't see, or isn't closed.
        "echo 'rm x' | sh",
        "curl x | bash",
        "base64 -d x | sh",
        "sh",
        "bash -s",
        "bash -c \"$CMD\"",
        "sh -c",
        "eval \"$CMD\"",
        "eval $CMD",
        "sudo bash -c \"$CMD\"",
        "python3 -c \"$CODE\"",
        "trap \"$CODE\" EXIT",
        "powershell -EncodedCommand AAAA",
        "cmd",
        "echo 'unclosed",
        "echo \"unclosed",
        "echo $(unclosed",
        "echo `unclosed",
        "echo ${unclosed",
        "(unclosed",
        "echo x \\",
        "sh <<EOF\nrm x",
        "cat <<EOF",
    ] {
        denied("rm", line);
        denied("git push", line);
    }
}

#[test]
fn a_deny_doesnt_catch_what_only_looks_like_it() {
    for line in [
        "echo rm",
        "echo 'rm x'",
        "echo \"rm x\"",
        "grep rm file.txt",
        "ls rm",
        "cat rm",
        "rmdir x",
        "trim x",
        "echo y # rm x",
        "# rm x",
        "git commit -m 'rm the old file'",
        "cat <<EOF\nrm x\nEOF",
        "command -v rm",
        "python3 script.py",
        "bash build.sh",
        "./build.sh --clean",
    ] {
        passes("rm", line);
    }
    passes("git push", "git status");
    passes("git push", "git pull origin main");
    passes("git push", "gh pr list");
    passes("git push", "npm run push");
}

#[test]
fn a_denys_words_are_matched_widely() {
    // Flags in any order, split or joined, long or short, case aside.
    for line in [
        "rm -rf x",
        "rm -fr x",
        "rm -r -f x",
        "rm x -r -f",
        "rm -Rf x",
        "rm --recursive --force x",
        "rm -v -rf x",
        "RM -RF x",
    ] {
        denied("rm -rf", line);
    }
    passes("rm -rf", "rm x");
    passes("rm -rf", "rm -f x");
    passes("rm -rf", "rm -r x");
    // A flag whose value could be anything, and words past a global flag.
    for line in [
        "git push",
        "git push origin main",
        "git -C /x push",
        "git --no-pager push",
        "git -c a=b push origin",
        "/usr/bin/git push",
        "FOO=1 git push",
        "git status && git push",
        "git $X",
        "git \"$X\" origin",
        "$GIT push",
    ] {
        denied("git push", line);
    }
    // `--force`: cut short, with a value, or by its short form.
    for line in [
        "git push --force",
        "git push origin main --force",
        "git push --forc",
        "git push -f",
        "git push --force=true",
    ] {
        denied("git push --force", line);
    }
    passes("git push --force", "git push origin main");
    // A glob could expand into anything, flags included.
    denied("rm -rf", "rm *");
    // `*` in a pattern.
    denied("npm run *", "npm run build");
    denied("terraform*", "terraform-1.5 apply");
    denied("*", "anything at all");
}

#[test]
fn an_allow_only_covers_what_it_literally_says() {
    allowed("git status", "git status");
    allowed("git status", "git status --short");
    allowed("git status", "/bin/zsh -lc 'git status'");
    allowed("git status", "bash -c \"git status --short\"");
    allowed("cargo test", "cargo test -p trek-core");
    allowed("ls", "ls *.rs");
    allowed("npm run *", "npm run build");
    for line in [
        "git status; rm -rf x",
        "git status && rm -rf x",
        "rm -rf x; git status",
        "git status | sh",
        "git status\nrm -rf x",
        "git status $(rm x)",
        "git status `rm x`",
        "git status $X",
        "git \"$X\"",
        "$X status",
        "\"$X\" git status",
        "git statuses",
        "git push",
        "/tmp/git status",
        "./git status",
        "PATH=/tmp git status",
        "GIT_DIR=/x git status",
        "sudo git status",
        "env git status",
        "command git status",
        "git status > ~/.zshrc",
        "git status >> x",
        "git status &> x",
        "git status 2> x",
        "bash --rcfile evil -c 'git status'",
        "sh -c \"$X\"",
        "git status '",
        "git status \"",
        "git status $(",
        "git status <<EOF",
        "FOO=1",
        "",
        "   ",
        "GIT status",
    ] {
        not_allowed("git status", line);
    }
    // Where its output goes doesn't matter when it goes nowhere.
    allowed("git status", "git status > /dev/null");
    allowed("git status", "git status 2>&1");
    allowed("git status", "git status 2>/dev/null");
    // Every command on the line needs its own.
    let rules = [allow("git add"), allow("git commit")];
    assert_eq!(
        action_for(&rules, "git add . && git commit -m x"),
        Some(RuleAction::Allow)
    );
    assert_eq!(action_for(&rules, "git add . && git push"), None);
    // What a command runs needs covering too, even if the command is allowed.
    not_allowed("find", "find . -exec rm {} ;");
    assert_eq!(
        action_for(&[allow("find"), allow("rm")], "find . -exec rm {} ;"),
        Some(RuleAction::Allow)
    );
}

#[test]
fn deny_wins_then_ask_then_allow_whatever_the_order() {
    let ask = rule(RuleKind::Command, "git", RuleAction::Ask);
    let yes = allow("git");
    let no = deny("git push");
    for rules in [
        vec![yes.clone(), ask.clone(), no.clone()],
        vec![no.clone(), ask.clone(), yes.clone()],
        vec![ask.clone(), yes.clone(), no.clone()],
    ] {
        assert_eq!(action_for(&rules, "git push"), Some(RuleAction::Deny));
        assert_eq!(action_for(&rules, "git status"), Some(RuleAction::Ask));
    }
    assert_eq!(
        action_for(&[yes.clone()], "git status"),
        Some(RuleAction::Allow)
    );
    assert_eq!(action_for(&[], "git status"), None);
    // The verdict says which rule, and what it caught.
    let v = decide(
        &[yes, no],
        &Call::command("bash", "echo hi; git push origin"),
        None,
    )
    .unwrap();
    assert_eq!(v.rule, 1);
    assert_eq!(v.part.as_deref(), Some("git push origin"));
    assert_eq!(v.unreadable, None);
    let v = decide(&[deny("rm")], &Call::command("bash", "echo 'x"), None).unwrap();
    assert!(v.unreadable.is_some(), "{v:?}");
}

#[test]
fn file_rules_read_a_path_the_way_a_write_would_land() {
    let dir = std::env::temp_dir().join(format!("trek-rules-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let root = dir.canonicalize().unwrap();
    let cwd = root.join("proj");
    std::fs::create_dir_all(cwd.join("src")).unwrap();
    std::fs::create_dir_all(root.join("secrets")).unwrap();
    let edit = |p: &str| Call::edit("Write", p);
    let on =
        |rules: &[PermissionRule], p: &str| decide(rules, &edit(p), Some(&cwd)).map(|v| v.action);
    let no = |pattern: &str| [rule(RuleKind::Edit, pattern, RuleAction::Deny)];
    let yes = |pattern: &str| [rule(RuleKind::Edit, pattern, RuleAction::Allow)];

    // A bare name, anywhere, as any spelling.
    for path in [".env", "src/.env", "./.env", "src/../.env", ".ENV"] {
        assert_eq!(on(&no(".env"), path), Some(RuleAction::Deny), "{path}");
    }
    assert_eq!(on(&no("*.lock"), "Cargo.lock"), Some(RuleAction::Deny));
    assert_eq!(on(&no(".env"), "src/env"), None);
    // A folder on the way counts for a deny.
    assert_eq!(
        on(&no("generated"), "src/generated/x.rs"),
        Some(RuleAction::Deny)
    );
    // A path, from the project.
    assert_eq!(on(&no("src/**"), "src/a/b.rs"), Some(RuleAction::Deny));
    assert_eq!(on(&no("src/"), "src/a/b.rs"), Some(RuleAction::Deny));
    assert_eq!(on(&no("src/a"), "src/a/b.rs"), Some(RuleAction::Deny));
    assert_eq!(on(&no("src/*.rs"), "src/b.rs"), Some(RuleAction::Deny));
    assert_eq!(on(&no("src/**"), "docs/b.rs"), None);
    assert_eq!(
        on(&no("src/**"), cwd.join("src/x.rs").to_str().unwrap()),
        Some(RuleAction::Deny),
        "an absolute path into the project"
    );
    // Out of the project by `..`, into a folder a rule names.
    let secrets = format!("{}/**", root.join("secrets").display());
    assert_eq!(on(&no(&secrets), "../secrets/key"), Some(RuleAction::Deny));
    assert_eq!(
        on(&no(&secrets), "src/../../secrets/key"),
        Some(RuleAction::Deny)
    );
    // Through a symlink that lands there.
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root.join("secrets"), cwd.join("innocent")).unwrap();
        assert_eq!(on(&no(&secrets), "innocent/key"), Some(RuleAction::Deny));
        assert_eq!(
            on(&yes("innocent/**"), "innocent/key"),
            None,
            "an allow must hold for where the write really lands"
        );
    }
    // Home.
    let home_rule = no("~/.ssh/**");
    let home_key = crate::paths::home().join(".ssh/id");
    assert_eq!(
        decide(&home_rule, &edit(home_key.to_str().unwrap()), None).map(|v| v.action),
        Some(RuleAction::Deny)
    );
    assert_eq!(
        decide(&home_rule, &edit("~/.ssh/id"), None).map(|v| v.action),
        Some(RuleAction::Deny)
    );
    // An allow is exact about case, and about what it names.
    assert_eq!(on(&yes("src/**"), "src/a.rs"), Some(RuleAction::Allow));
    assert_eq!(on(&yes("src/**"), "SRC/a.rs"), None);
    assert_eq!(on(&yes("src/**"), "src/../../a.rs"), None);
    assert_eq!(on(&yes("*.md"), "docs/a.md"), Some(RuleAction::Allow));
    assert_eq!(
        on(&yes("docs"), "docs/a.md"),
        None,
        "a bare name is the file's"
    );
    // No path given: a deny holds, an allow doesn't.
    assert_eq!(on(&no("src/**"), ""), Some(RuleAction::Deny));
    assert_eq!(on(&yes("**"), ""), None);
    // Rules of one kind leave other kinds alone.
    assert_eq!(decide(&no("*"), &Call::command("bash", "rm x"), None), None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fetch_rules_read_the_host_a_client_would_reach() {
    let fetch = |u: &str| Call::fetch("WebFetch", u);
    let no = |p: &str| [rule(RuleKind::Fetch, p, RuleAction::Deny)];
    let yes = |p: &str| [rule(RuleKind::Fetch, p, RuleAction::Allow)];
    let on = |r: &[PermissionRule], u: &str| decide(r, &fetch(u), None).map(|v| v.action);
    for url in [
        "https://evil.com/",
        "http://EVIL.com",
        "https://evil.com:8443/x",
        "https://user:pw@evil.com/x",
        "https://good.com@evil.com/",
        "https://sub.evil.com/",
        "https://evil.com./",
        "evil.com/path",
    ] {
        assert_eq!(on(&no("evil.com"), url), Some(RuleAction::Deny), "{url}");
    }
    assert_eq!(on(&no("evil.com"), "https://notevil.com/"), None);
    assert_eq!(on(&no("evil.com"), "https://evil.com.good.org/"), None);
    assert_eq!(
        on(&no("*.corp"), "https://db.corp/"),
        Some(RuleAction::Deny)
    );
    assert_eq!(
        on(&no("example.com/admin/*"), "https://example.com/admin/x"),
        Some(RuleAction::Deny)
    );
    assert_eq!(
        on(&no("example.com/admin/*"), "https://example.com/docs"),
        None
    );
    // Something that isn't an address: a deny holds, an allow doesn't.
    assert_eq!(
        on(&no("example.com"), "not an address"),
        Some(RuleAction::Deny)
    );
    assert_eq!(on(&yes("example.com"), "not an address"), None);
    assert_eq!(on(&yes("example.com"), ""), None);
    // An allow is for that host only.
    assert_eq!(
        on(&yes("example.com"), "https://example.com/x"),
        Some(RuleAction::Allow)
    );
    assert_eq!(on(&yes("example.com"), "https://a.example.com/x"), None);
    assert_eq!(
        on(&yes("example.com"), "https://example.com@evil.com/"),
        None
    );
    assert_eq!(
        on(&yes("*.example.com"), "https://a.example.com/x"),
        Some(RuleAction::Allow)
    );
}

#[test]
fn tool_rules_go_by_the_agents_name_for_it() {
    let no = [rule(RuleKind::Tool, "mcp__github__*", RuleAction::Deny)];
    let call = Call::tool("mcp__github__create_issue");
    assert_eq!(
        decide(&no, &call, None).map(|v| v.action),
        Some(RuleAction::Deny)
    );
    assert_eq!(decide(&no, &Call::tool("mcp__other__x"), None), None);
    // A tool rule about the shell or an edit tool holds for what they do too.
    let bash = [rule(RuleKind::Tool, "bash", RuleAction::Deny)];
    assert_eq!(
        decide(&bash, &Call::command("Bash", "ls"), None).map(|v| v.action),
        Some(RuleAction::Deny)
    );
    let shell = [rule(RuleKind::Tool, "Bash", RuleAction::Allow)];
    assert_eq!(
        decide(&shell, &Call::command("Bash", "ls; rm x"), None).map(|v| v.action),
        Some(RuleAction::Allow),
        "the whole tool, as asked"
    );
    assert_eq!(
        decide(&shell, &Call::command("Bash", "echo 'unclosed"), None),
        None,
        "but not what can't be read"
    );
    assert_eq!(decide(&shell, &Call::command("Bash", ""), None), None);
    let write = [rule(RuleKind::Tool, "Write", RuleAction::Allow)];
    assert_eq!(
        decide(&write, &Call::edit("Write", "a.rs"), None).map(|v| v.action),
        Some(RuleAction::Allow)
    );
    // Nameless: a deny holds, an allow doesn't.
    let any = [rule(RuleKind::Tool, "x", RuleAction::Deny)];
    assert_eq!(
        decide(&any, &Call::tool(""), None).map(|v| v.action),
        Some(RuleAction::Deny)
    );
    let yes = [rule(RuleKind::Tool, "*", RuleAction::Allow)];
    assert_eq!(decide(&yes, &Call::tool(""), None), None);
}

#[test]
fn an_empty_command_counts_for_a_deny_and_not_for_an_allow() {
    assert_eq!(action_for(&[deny("rm")], ""), Some(RuleAction::Deny));
    assert_eq!(action_for(&[allow("rm")], ""), None);
}

#[test]
fn a_line_built_to_exhaust_the_parser_is_just_unreadable() {
    let deep = format!("{}rm x{}", "$(".repeat(100_000), ")".repeat(100_000));
    assert_eq!(action_for(&[deny("git")], &deep), Some(RuleAction::Deny));
    // Each level quadruples the quoting, so ten is plenty past the limit.
    let shells = (0..10).fold("rm x".to_string(), |line, _| {
        format!("sh -c {}", shlex_quote(&line))
    });
    assert_eq!(action_for(&[deny("git")], &shells), Some(RuleAction::Deny));
    assert_eq!(action_for(&[allow("sh")], &shells), None);
}

fn shlex_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[test]
fn outcomes_at_each_level_put_the_rule_on_top_of_the_level() {
    let asks = |h: HandHolding| h != HandHolding::FullAccess;
    let at = |rules: &[PermissionRule]| {
        outcome_at(rules, &Call::command("bash", "rm x"), None, asks)
            .into_iter()
            .map(|(_, o)| o)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        at(&[deny("rm")]),
        vec![Outcome::Refused; 4],
        "Full access obeys it"
    );
    assert_eq!(
        at(&[rule(RuleKind::Command, "rm", RuleAction::Ask)]),
        vec![Outcome::Asks; 4]
    );
    assert_eq!(at(&[allow("rm")]), vec![Outcome::Runs; 4]);
    assert_eq!(
        at(&[]),
        vec![Outcome::Asks, Outcome::Asks, Outcome::Asks, Outcome::Runs]
    );
}

#[test]
fn always_allow_offers_the_narrowest_useful_rule_or_none() {
    let cmd = |line: &str| allow_draft(&Call::command("bash", line), None).map(|r| r.pattern);
    assert_eq!(cmd("git push origin main").as_deref(), Some("git push"));
    assert_eq!(cmd("git --no-pager log").as_deref(), Some("git"));
    assert_eq!(
        cmd("npm run test -- --watch").as_deref(),
        Some("npm run test")
    );
    assert_eq!(cmd("cargo test -p x").as_deref(), Some("cargo test"));
    assert_eq!(
        cmd("/bin/zsh -lc 'cargo test'").as_deref(),
        Some("cargo test")
    );
    // A program Trek knows nothing about: as written, for the user to shorten.
    assert_eq!(cmd("rm -rf build").as_deref(), Some("rm -rf build"));
    assert_eq!(
        cmd("./scripts/migrate.sh --apply").as_deref(),
        Some("./scripts/migrate.sh --apply")
    );
    // No single rule could cover these.
    for line in [
        "cd x && cargo test",
        "git status; rm x",
        "echo $(date)",
        "FOO=1 npm test",
        "echo x > file",
        "$X y",
        "echo 'unclosed",
        "",
    ] {
        assert_eq!(cmd(line), None, "{line}");
    }
    // The draft really is what covers it.
    for line in [
        "git push origin main",
        "rm -rf build",
        "npm run test -- --watch",
    ] {
        let call = Call::command("bash", line);
        let draft = allow_draft(&call, None).unwrap();
        assert_eq!(
            decide(std::slice::from_ref(&draft), &call, None).map(|v| v.action),
            Some(RuleAction::Allow),
            "{line}"
        );
    }
    let cwd = Path::new("/proj");
    let edit = |p: &str| allow_draft(&Call::edit("Write", p), Some(cwd)).map(|r| r.pattern);
    assert_eq!(edit("src/a.rs").as_deref(), Some("src/a.rs"));
    assert_eq!(edit("/proj/src/a.rs").as_deref(), Some("src/a.rs"));
    assert_eq!(edit("").as_deref(), None);
    assert_eq!(
        allow_draft(&Call::fetch("WebFetch", "https://a.example.com/x"), None)
            .map(|r| r.pattern)
            .as_deref(),
        Some("a.example.com")
    );
    assert_eq!(
        allow_draft(&Call::tool("WebSearch"), None)
            .map(|r| r.pattern)
            .as_deref(),
        Some("WebSearch")
    );
}

#[test]
fn a_rule_that_cant_mean_anything_says_so() {
    assert!(problem(RuleKind::Command, "  ").is_some());
    assert!(problem(RuleKind::Command, "-rf").is_some());
    assert!(problem(RuleKind::Command, "rm; echo").is_some());
    assert!(problem(RuleKind::Command, "git push").is_none());
    assert!(problem(RuleKind::Edit, "").is_some());
    assert!(problem(RuleKind::Edit, "~/.ssh/**").is_none());
}

#[test]
fn rules_round_trip_through_settings() {
    let mut s = crate::settings::Settings::default();
    s.permissions.rules = vec![
        deny("rm"),
        rule(RuleKind::Edit, "~/.ssh/**", RuleAction::Ask),
    ];
    let text = toml::to_string_pretty(&s).unwrap();
    assert!(text.contains("[[permissions.rules]]"), "{text}");
    assert_eq!(
        toml::from_str::<crate::settings::Settings>(&text).unwrap(),
        s
    );
    // Files from before rules: none.
    let old: crate::settings::Settings =
        toml::from_str("[permissions]\nfull_access_unlocked = true\n").unwrap();
    assert!(old.permissions.rules.is_empty() && old.permissions.full_access_unlocked);
}

#[test]
fn only_a_rule_that_stops_something_needs_every_step_seen() {
    assert!(!watches_work(&[]));
    assert!(!watches_work(&[allow("git status")]));
    assert!(watches_work(&[deny("rm")]));
    assert!(watches_work(&[rule(
        RuleKind::Edit,
        "*.lock",
        RuleAction::Ask
    )]));
    assert!(watches_work(&[rule(
        RuleKind::Tool,
        "Bash",
        RuleAction::Deny
    )]));
}

#[test]
fn what_is_shown_is_clipped_and_on_one_line() {
    let note = stopped_note(&deny("rm"), &Call::command("bash", "rm x\n  y"));
    assert_eq!(note, "Your rule stopped `rm x y` (Deny command `rm`).");
    let long = stopped_note(&deny("rm"), &Call::command("bash", "rm ".repeat(100)));
    assert!(long.contains('…') && long.len() < 200, "{long}");
    assert!(refusal(&deny("rm")).contains("Deny command `rm`"));
}

/// cmd.exe's own reading, for what `cmd /c` runs and for a line Trek runs in cmd.exe (its own
/// agent on a PC without Git Bash): `^` keeps the next character as it is, a command's name ends
/// at `/`, a leading `@` only hides the echo, `if`, `for`, `call` and `start` run what follows,
/// and `%X%` is only known when it runs. The Mac's escapes (`\rm`, `'r'm`) are undone the same way.
#[test]
fn cmd_lines_are_read_as_cmd_exe_reads_them() {
    for line in [
        "cmd /c d^el x",
        "cmd /c d^e^l x",
        "cmd /c \"d^el x\"",
        "cmd.exe /C del/q x",
        "cmd /c @del x",
        "cmd /c echo a&&del x",
        "cmd /c (del x)",
        "cmd /c if exist x del x",
        "cmd /c for %f in (*) do del %f",
        "cmd /c call del x",
        "cmd /c start \"\" del x",
        "cmd //c del x",
        "cmd /d /s /c \"del x\"",
        "cmd /c %X% y",
        "cmd /c !X! y",
        "cmd /c cmd /c d^el x",
        "cmd /c powershell -c \"del x\"",
    ] {
        denied("del", line);
    }

    let in_cmd = |rules: &[PermissionRule], line: &str| {
        decide(rules, &Call::command_in_cmd("bash", line), None).map(|v| v.action)
    };
    for line in [
        "d^el x",
        "del/q x",
        "@del x",
        "echo a & del x",
        "echo a | del x",
        "(echo a) && del x",
        "if not exist y del x",
        "for %f in (*.tmp) do @del %f",
        "%X% y",
        "echo a > out.txt & d^el x",
    ] {
        assert_eq!(
            in_cmd(&[deny("del")], line),
            Some(RuleAction::Deny),
            "`{line}` must not get past a deny on `del` in cmd.exe"
        );
    }
    // An escaped or quoted separator is only text; a lone `%` is a percent sign.
    for line in [
        "echo a ^& del x",
        "echo \"a & del x\"",
        "echo del",
        "echo 100% sure",
        "dir /w",
    ] {
        assert_eq!(in_cmd(&[deny("del")], line), None, "`{line}` runs no del");
    }
    // An allow covers a line with its escapes undone, and nothing only known when it runs.
    assert_eq!(
        in_cmd(&[allow("git status")], "git st^atus"),
        Some(RuleAction::Allow)
    );
    assert_eq!(in_cmd(&[allow("git status")], "git status & del x"), None);
    assert_eq!(in_cmd(&[allow("git status")], "%G% status"), None);
    // The same line read as a POSIX shell's: there `d^el` is a program of that name.
    passes("del", "d^el x");
}
