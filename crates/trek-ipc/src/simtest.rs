//! iOS test runs as the Simulator tool follows them: `xcodebuild test` output, read a line at a
//! time into test cases and how each ended (XCTest, serial and parallel, and Swift Testing).
//!
//! A run started with the simulator server's `sim_test` tool is written as it goes to a log in
//! [`ENV_RUNS_DIR`] (Trek hands the server the folder): a [`HEADER`] line with the run's JSON,
//! then `xcodebuild`'s own lines, then a [`FOOTER`] line with its exit code. The Simulator tool
//! tails the newest one, so the user watches the tests pass and fail while they run.

/// The folder `sim_test` writes its run logs to.
pub const ENV_RUNS_DIR: &str = "TREK_SIM_RUNS";
/// A run log's first line: this, then the run's [`RunHeader`] as JSON.
pub const HEADER: &str = "#trek-sim-test ";
/// A run log's last line: this, then `xcodebuild`'s exit code.
pub const FOOTER: &str = "#trek-sim-test-exit ";

#[derive(Debug, Clone, PartialEq)]
pub struct RunHeader {
    /// What the run is called on screen: its scheme, or the command.
    pub label: String,
    /// The device it runs on.
    pub udid: String,
    pub started_ms: i64,
}

impl RunHeader {
    /// The header line, newline included.
    pub fn line(&self) -> String {
        let json = serde_json::json!({
            "label": self.label,
            "udid": self.udid,
            "started_ms": self.started_ms,
        });
        format!("{HEADER}{json}\n")
    }

    /// A header line read back.
    pub fn parse(line: &str) -> Option<Self> {
        let v: serde_json::Value =
            serde_json::from_str(line.trim().strip_prefix(HEADER.trim_end())?.trim()).ok()?;
        Some(Self {
            label: v["label"].as_str()?.to_string(),
            udid: v["udid"].as_str().unwrap_or_default().to_string(),
            started_ms: v["started_ms"].as_i64().unwrap_or_default(),
        })
    }
}

/// The exit code a footer line gives, when it is one.
pub fn footer_code(line: &str) -> Option<i32> {
    line.trim()
        .strip_prefix(FOOTER.trim_end())?
        .trim()
        .parse()
        .ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseStatus {
    Running,
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Case {
    /// The test's class or suite (empty for a free Swift Testing function).
    pub suite: String,
    pub name: String,
    pub status: CaseStatus,
    pub seconds: Option<f64>,
    /// Why it failed (its first reported failure).
    pub message: Option<String>,
}

impl Case {
    /// `Suite › name`, or the name alone.
    pub fn title(&self) -> String {
        if self.suite.is_empty() {
            self.name.clone()
        } else {
            format!("{} › {}", self.suite, self.name)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    /// Building, before the first test starts.
    #[default]
    Building,
    Testing,
    Passed,
    Failed,
}

impl Phase {
    pub fn done(self) -> bool {
        matches!(self, Phase::Passed | Phase::Failed)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TestRun {
    pub label: String,
    pub cases: Vec<Case>,
    pub phase: Phase,
    /// The last `Executed N tests, …` line.
    pub summary: Option<String>,
    /// Errors that stopped it before the tests ran (a build that failed), the first few.
    pub errors: Vec<String>,
    in_failure_list: bool,
}

/// Errors kept for a run that never got to its tests.
const MAX_ERRORS: usize = 6;

impl TestRun {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            ..Self::default()
        }
    }

    pub fn feed_all(&mut self, text: &str) {
        for line in text.lines() {
            self.feed(line);
        }
    }

    /// Read one line of `xcodebuild test` output.
    pub fn feed(&mut self, line: &str) {
        let line = line.trim_end_matches('\r');
        let t = line.trim();
        if t.is_empty() {
            return;
        }
        // `Testing failed:` is followed by its reasons, a tab in.
        if self.in_failure_list {
            if line.starts_with('\t') || line.starts_with("    ") {
                if self.cases.is_empty() {
                    self.push_error(t);
                }
                return;
            }
            self.in_failure_list = false;
        }
        match t {
            "** TEST SUCCEEDED **" => {
                self.phase = Phase::Passed;
                return;
            }
            "** TEST FAILED **" | "** BUILD FAILED **" | "** TEST INTERRUPTED **" => {
                self.phase = Phase::Failed;
                return;
            }
            "Testing failed:" => {
                self.in_failure_list = true;
                return;
            }
            "Testing started" => {
                self.start_testing();
                return;
            }
            _ => {}
        }
        if let Some(rest) = t
            .strip_prefix("Test Case '")
            .or_else(|| t.strip_prefix("Test case '"))
        {
            return self.xctest_line(rest);
        }
        if t.starts_with("Test Suite '") {
            if t.contains("' started at") {
                self.start_testing();
            }
            return;
        }
        if t.starts_with("Executed ") && t.contains(" test") {
            self.summary = Some(t.to_string());
            return;
        }
        if self.swift_testing_line(t) {
            return;
        }
        // `/path/File.swift:7: error: -[Module.Class testName] : message`
        if let Some((_, after)) = t.split_once(": error: ") {
            if let Some(rest) = after.strip_prefix("-[") {
                if let Some((id, message)) = rest.split_once("] : ") {
                    let (suite, name) = case_id(&format!("-[{id}]"));
                    let case = self.upsert(&suite, &name);
                    if case.message.is_none() {
                        case.message = Some(message.trim().to_string());
                    }
                    return;
                }
            }
            if self.phase == Phase::Building {
                self.push_error(t);
            }
        }
    }

    /// The run is over: `ok` when it exited cleanly. Tests still running then never finished.
    pub fn finish(&mut self, ok: bool) {
        if !self.phase.done() {
            self.phase = if ok { Phase::Passed } else { Phase::Failed };
        }
        for case in &mut self.cases {
            if case.status == CaseStatus::Running {
                case.status = CaseStatus::Failed;
                case.message.get_or_insert_with(|| "Didn't finish".into());
            }
        }
    }

    /// The test running now (the latest one started).
    pub fn current(&self) -> Option<&Case> {
        self.cases
            .iter()
            .rev()
            .find(|c| c.status == CaseStatus::Running)
    }

    /// Passed, failed and skipped so far.
    pub fn counts(&self) -> (usize, usize, usize) {
        let n = |s: CaseStatus| self.cases.iter().filter(|c| c.status == s).count();
        (
            n(CaseStatus::Passed),
            n(CaseStatus::Failed),
            n(CaseStatus::Skipped),
        )
    }

    /// The run for an agent: how it ended, its counts and what failed.
    pub fn report(&self) -> String {
        let (passed, failed, skipped) = self.counts();
        let verdict = match self.phase {
            Phase::Passed => "Tests passed",
            Phase::Failed if self.cases.is_empty() => "The run failed before any test ran",
            Phase::Failed => "Tests failed",
            _ => "Tests still running",
        };
        let mut out = format!("{verdict}: {passed} passed, {failed} failed, {skipped} skipped.");
        let failures: Vec<&Case> = self
            .cases
            .iter()
            .filter(|c| c.status == CaseStatus::Failed)
            .collect();
        if !failures.is_empty() {
            out.push_str("\nFailed:");
            for c in failures {
                out.push_str(&format!("\n- {}", c.title()));
                if let Some(m) = &c.message {
                    out.push_str(&format!(": {m}"));
                }
            }
        }
        if !self.errors.is_empty() {
            out.push_str("\nErrors:");
            for e in &self.errors {
                out.push_str(&format!("\n- {e}"));
            }
        }
        out
    }

    fn start_testing(&mut self) {
        if self.phase == Phase::Building {
            self.phase = Phase::Testing;
        }
    }

    fn push_error(&mut self, e: &str) {
        if self.errors.len() < MAX_ERRORS && !self.errors.iter().any(|x| x == e) {
            self.errors.push(e.to_string());
        }
    }

    fn upsert(&mut self, suite: &str, name: &str) -> &mut Case {
        let ix = match self
            .cases
            .iter()
            .rposition(|c| c.suite == suite && c.name == name)
        {
            Some(ix) => ix,
            None => {
                self.cases.push(Case {
                    suite: suite.to_string(),
                    name: name.to_string(),
                    status: CaseStatus::Running,
                    seconds: None,
                    message: None,
                });
                self.cases.len() - 1
            }
        };
        &mut self.cases[ix]
    }

    /// After `Test Case '`: `-[M.C test]' started.`, `-[M.C test]' passed (1.5 seconds).`, or in
    /// parallel runs `-[M.C test]' failed on 'Clone 1 of iPhone 17 …' (0.5 seconds)`.
    fn xctest_line(&mut self, rest: &str) {
        let Some((id, outcome)) = rest.split_once("' ") else {
            return;
        };
        let (suite, name) = case_id(id);
        if name.is_empty() {
            return;
        }
        self.start_testing();
        let word = outcome.split([' ', '.']).next().unwrap_or_default();
        let status = match word {
            "started" => {
                let case = self.upsert(&suite, &name);
                case.status = CaseStatus::Running;
                return;
            }
            "passed" => CaseStatus::Passed,
            "failed" => CaseStatus::Failed,
            "skipped" => CaseStatus::Skipped,
            _ => return,
        };
        let seconds = seconds_in(outcome);
        let case = self.upsert(&suite, &name);
        case.status = status;
        case.seconds = seconds;
    }

    /// Swift Testing's own lines: `◇ Test name() started.`, `✔ Test name() passed after 0.1
    /// seconds.`, `✘ Test name() failed after …`, `✘ Test name() recorded an issue at F.swift:3:5:
    /// message`, `➜ Test name() skipped…`. Whether the line was one.
    fn swift_testing_line(&mut self, t: &str) -> bool {
        let Some(rest) = [
            "◇ Test ",
            "✔ Test ",
            "✘ Test ",
            "➜ Test ",
            "↳ Test ",
            "􀟈 Test ",
        ]
        .iter()
        .find_map(|p| t.strip_prefix(p)) else {
            return false;
        };
        let Some(end) = rest
            .find(") ")
            .or_else(|| rest.strip_suffix(')').map(|r| r.len()))
        else {
            return false;
        };
        let name = rest[..=end].to_string();
        let outcome = rest[end + 1..].trim();
        self.start_testing();
        let case = self.upsert("", &name);
        if outcome.starts_with("started") {
            case.status = CaseStatus::Running;
        } else if outcome.starts_with("passed") {
            case.status = CaseStatus::Passed;
            case.seconds = seconds_in(outcome);
        } else if outcome.starts_with("skipped") {
            case.status = CaseStatus::Skipped;
        } else if let Some(issue) = outcome.strip_prefix("recorded an issue") {
            // ` at File.swift:12:5: Expectation failed: …`
            let message = issue
                .split_once(": ")
                .map(|(_, m)| m.trim())
                .unwrap_or(issue.trim());
            if case.message.is_none() && !message.is_empty() {
                case.message = Some(message.to_string());
            }
        } else if outcome.starts_with("failed") {
            case.status = CaseStatus::Failed;
            case.seconds = seconds_in(outcome);
        }
        true
    }
}

/// A test's class (or suite) and name from how `xcodebuild` names it: `-[Module.Class testName]`,
/// `Module.Class/testName()`, `Class.testName()` or `Suite/name()`.
pub fn case_id(id: &str) -> (String, String) {
    let id = id.trim();
    if let Some(inner) = id.strip_prefix("-[").and_then(|r| r.strip_suffix(']')) {
        let (class, name) = inner.split_once(' ').unwrap_or(("", inner));
        let class = class.rsplit('.').next().unwrap_or(class);
        return (class.to_string(), name.trim().to_string());
    }
    let split = id
        .rfind('/')
        .or_else(|| id.find('(').and_then(|p| id[..p].rfind('.')));
    match split {
        Some(i) => {
            let suite = &id[..i];
            let suite = suite.rsplit('.').next().unwrap_or(suite);
            (suite.to_string(), id[i + 1..].to_string())
        }
        None => (String::new(), id.to_string()),
    }
}

/// `(1.503 seconds)` or `after 0.25 seconds`: the number of seconds.
fn seconds_in(s: &str) -> Option<f64> {
    let before = s.split(" seconds").next()?;
    let number = before.rsplit(['(', ' ']).next()?.trim_start_matches('(');
    number.parse().ok()
}

/// What a shell command run by an agent does to a simulator, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimCommand {
    /// Drives a simulator (`simctl` boot, launch, io…, AXe).
    Drive,
    /// Runs tests (`xcodebuild test` and its kin), with a label for the run.
    Test(String),
}

/// Recognise simulator work in an agent's shell command line.
pub fn sim_command(command: &str) -> Option<SimCommand> {
    let mut found = None;
    for segment in command.split(['\n', ';', '|', '&']) {
        let words: Vec<&str> = segment.split_whitespace().collect();
        // Past `VAR=value`, `env`, `time`, `xcrun`.
        let start = words
            .iter()
            .position(|w| {
                !(w.contains('=') && !w.starts_with('-'))
                    && !matches!(
                        w.rsplit('/').next().unwrap_or(w),
                        "env" | "time" | "command" | "exec" | "nohup" | "xcrun"
                    )
            })
            .unwrap_or(words.len());
        let words = &words[start..];
        let Some(cmd) = words.first() else { continue };
        let cmd = cmd.rsplit('/').next().unwrap_or(cmd);
        match cmd {
            "xcodebuild"
                if words[1..]
                    .iter()
                    .any(|w| matches!(*w, "test" | "test-without-building")) =>
            {
                let flag = |name: &str| {
                    words
                        .iter()
                        .position(|w| *w == name)
                        .and_then(|i| words.get(i + 1))
                        .map(|v| v.trim_matches(['\'', '"']).to_string())
                };
                let label = flag("-scheme")
                    .or_else(|| flag("-testPlan"))
                    .unwrap_or_else(|| "xcodebuild test".into());
                return Some(SimCommand::Test(label));
            }
            "simctl"
                if words
                    .get(1)
                    .is_some_and(|w| !matches!(*w, "list" | "help" | "--help" | "-h")) =>
            {
                found = Some(SimCommand::Drive)
            }
            "axe" if words.len() > 1 => found = Some(SimCommand::Drive),
            _ => {}
        }
    }
    found
}

/// Whether an MCP tool call is Trek's simulator server's (`mcp__trek-simulator__sim_tap`, or
/// another agent's spelling of it).
pub fn is_sim_tool(title: &str) -> bool {
    title.contains("trek-simulator") && title.contains("sim_")
}

#[cfg(test)]
mod tests {
    use super::*;

    const XCTEST: &str = "\
CompileSwift normal arm64 /tmp/x.swift
Testing started
Test Suite 'All tests' started at 2026-10-26 20:48:10.253.
Test Suite 'MathTests' started at 2026-10-26 20:48:10.257.
Test Case '-[SampleTests.MathTests testAdds]' started.
Test Case '-[SampleTests.MathTests testAdds]' passed (1.503 seconds).
Test Case '-[SampleTests.MathTests testFailsOnPurpose]' started.
/tmp/s/Tests/SampleTests.swift:7: error: -[SampleTests.MathTests testFailsOnPurpose] : XCTAssertEqual failed: (\"2\") is not equal to (\"3\") - deliberate failure
Test Case '-[SampleTests.MathTests testFailsOnPurpose]' failed (1.872 seconds).
Test Suite 'MathTests' failed at 2026-10-26 20:48:15.141.
\t Executed 2 tests, with 1 failure (0 unexpected) in 3.3 (3.3) seconds
Test Case '-[SampleTests.SlowTests testSkips]' started.
Test Case '-[SampleTests.SlowTests testSkips]' skipped (0.013 seconds).
Test Case '-[SampleTests.SlowTests testWaitsABit]' started.
";

    #[test]
    fn xctest_cases_follow_their_lines() {
        let mut run = TestRun::new("Sample");
        run.feed_all(XCTEST);
        assert_eq!(run.phase, Phase::Testing);
        assert_eq!(run.counts(), (1, 1, 1));
        assert_eq!(
            run.current().map(|c| c.title()).as_deref(),
            Some("SlowTests › testWaitsABit")
        );
        let failed = &run.cases[1];
        assert_eq!(failed.status, CaseStatus::Failed);
        assert_eq!(failed.seconds, Some(1.872));
        assert!(
            failed
                .message
                .as_deref()
                .unwrap()
                .starts_with("XCTAssertEqual failed")
        );
        assert!(
            run.summary
                .as_deref()
                .unwrap()
                .starts_with("Executed 2 tests")
        );

        run.feed("Test Case '-[SampleTests.SlowTests testWaitsABit]' passed (2.001 seconds).");
        run.feed("** TEST FAILED **");
        assert_eq!(run.phase, Phase::Failed);
        assert_eq!(run.counts(), (2, 1, 1));
        assert!(run.current().is_none());
        let report = run.report();
        assert!(
            report.starts_with("Tests failed: 2 passed, 1 failed, 1 skipped."),
            "{report}"
        );
        assert!(
            report.contains("- MathTests › testFailsOnPurpose: XCTAssertEqual"),
            "{report}"
        );
    }

    #[test]
    fn parallel_and_swift_testing_lines() {
        let mut run = TestRun::new("App");
        run.feed("Test case '-[AppTests.LoginTests testSignIn]' passed on 'Clone 1 of iPhone 17 - AppTests (4242)' (0.512 seconds)");
        run.feed("Test case 'ParserTests/parsesEmpty()' failed on 'Clone 2 of iPhone 17 - AppTests (4243)' (0.020 seconds)");
        run.feed("◇ Test addsNumbers() started.");
        assert_eq!(
            run.current().map(|c| c.name.as_str()),
            Some("addsNumbers()")
        );
        run.feed("✘ Test addsNumbers() recorded an issue at MathTests.swift:12:5: Expectation failed: (sum(1, 1) → 2) == 3");
        run.feed("✘ Test addsNumbers() failed after 0.003 seconds with 1 issue.");
        run.feed("◇ Test later() started.");
        run.feed("➜ Test later() skipped: \"not yet\"");
        assert_eq!(run.counts(), (1, 2, 1));
        assert_eq!(run.cases[0].suite, "LoginTests");
        assert_eq!(run.cases[1].suite, "ParserTests");
        assert_eq!(run.cases[1].name, "parsesEmpty()");
        assert_eq!(
            run.cases[2].message.as_deref(),
            Some("Expectation failed: (sum(1, 1) → 2) == 3")
        );
        assert_eq!(run.cases[2].seconds, Some(0.003));
    }

    #[test]
    fn a_build_that_fails_keeps_its_errors() {
        let mut run = TestRun::new("Sample");
        run.feed("/tmp/s/Tests/T.swift:5:42: error: cannot find 'sum' in scope");
        run.feed("Testing failed:");
        run.feed("\tCannot find 'sum' in scope");
        run.feed("\tTesting cancelled because the build failed.");
        run.feed("** TEST FAILED **");
        assert_eq!(run.phase, Phase::Failed);
        assert!(run.cases.is_empty());
        assert_eq!(run.errors.len(), 3);
        assert!(
            run.report()
                .starts_with("The run failed before any test ran")
        );
    }

    #[test]
    fn an_interrupted_run_fails_what_was_running() {
        let mut run = TestRun::new("Sample");
        run.feed("Test Case '-[A.B testC]' started.");
        run.finish(false);
        assert_eq!(run.phase, Phase::Failed);
        assert_eq!(run.cases[0].status, CaseStatus::Failed);
        let mut ok = TestRun::new("Sample");
        ok.finish(true);
        assert_eq!(ok.phase, Phase::Passed);
    }

    #[test]
    fn run_logs_open_and_close_with_their_lines() {
        let h = RunHeader {
            label: "Sample".into(),
            udid: "ABC".into(),
            started_ms: 42,
        };
        assert_eq!(RunHeader::parse(&h.line()), Some(h));
        assert_eq!(footer_code(&format!("{FOOTER}65")), Some(65));
        assert_eq!(footer_code("Test Case"), None);
        assert_eq!(RunHeader::parse("Testing started"), None);
    }

    #[test]
    fn case_ids_in_every_spelling() {
        let id = |s: &str| case_id(s);
        assert_eq!(id("-[Mod.Class testX]"), ("Class".into(), "testX".into()));
        assert_eq!(id("Mod.Class/testX()"), ("Class".into(), "testX()".into()));
        assert_eq!(id("Class.testX()"), ("Class".into(), "testX()".into()));
        assert_eq!(id("free()"), ("".into(), "free()".into()));
    }

    #[test]
    fn shell_commands_that_drive_a_simulator() {
        let c = sim_command;
        assert_eq!(
            c(
                "cd ios && xcodebuild -project Trek.xcodeproj -scheme Trek -destination 'platform=iOS Simulator,name=iPhone 17' test"
            ),
            Some(SimCommand::Test("Trek".into()))
        );
        assert_eq!(
            c("xcodebuild test-without-building -xctestrun x.xctestrun"),
            Some(SimCommand::Test("xcodebuild test".into()))
        );
        assert_eq!(c("xcrun simctl boot 'iPhone 17'"), Some(SimCommand::Drive));
        assert_eq!(
            c("/usr/bin/xcrun simctl io booted screenshot a.png"),
            Some(SimCommand::Drive)
        );
        assert_eq!(c("axe tap -x 10 -y 20 --udid X"), Some(SimCommand::Drive));
        assert_eq!(c("xcrun simctl list devices"), None);
        assert_eq!(c("xcodebuild -scheme Trek build"), None);
        assert_eq!(c("grep simctl notes.md"), None);
        assert!(is_sim_tool("mcp__trek-simulator__sim_tap"));
        assert!(!is_sim_tool("mcp__trek-computer__click"));
    }
}
