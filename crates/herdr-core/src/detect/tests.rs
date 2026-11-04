//! Engine tests: synthetic manifests for gate/region/priority semantics,
//! the bundled manifests against captured screens.

use super::{manifest::Manifest, *};

fn input(screen: &str) -> Input<'_> {
    Input { screen, title: "", progress: "" }
}

fn titled<'a>(screen: &'a str, title: &'a str) -> Input<'a> {
    Input { screen, title, progress: "" }
}

fn compile(text: &str) -> Manifest {
    Manifest::compile(text).unwrap_or_else(|e| panic!("manifest should compile: {e}"))
}

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/detect").join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{name}: {e}"))
}

// --- gate semantics -------------------------------------------------------

#[test]
fn a_gate_is_and_of_matchers_with_nested_any_and_not() {
    let m = compile(
        r##"
        id = "t"
        [[rules]]
        id = "r"
        state = "working"
        contains = ["Alpha"]
        any = [{ contains = ["beta"] }, { contains = ["gamma"] }]
        not = [{ contains = ["blocked word"] }]
        "##,
    );
    assert_eq!(detect(&m, &input("ALPHA beta")).state, State::Working, "contains is case-insensitive");
    assert_eq!(detect(&m, &input("alpha gamma")).state, State::Working);
    assert_eq!(detect(&m, &input("alpha")).state, State::Unknown, "any branch must match");
    assert_eq!(detect(&m, &input("alpha beta blocked word")).state, State::Unknown, "not wins over all");
}

#[test]
fn regex_needs_every_pattern_line_regex_needs_each_on_some_line() {
    let m = compile(
        r##"
        id = "t"
        [[rules]]
        id = "r"
        state = "idle"
        regex = ["foo", "bar"]
        line_regex = ["^\\s*> "]
        "##,
    );
    assert_eq!(detect(&m, &input("foo xx bar\n> prompt")).state, State::Idle);
    assert_eq!(detect(&m, &input("foo only\n> prompt")).state, State::Unknown);
    assert_eq!(detect(&m, &input("foo bar\nno prompt")).state, State::Unknown);
}

#[test]
fn an_empty_any_passes_and_nested_gates_compose() {
    let m = compile(
        r##"
        id = "t"
        [[rules]]
        id = "r"
        state = "blocked"
        all = [{ contains = ["a"] }, { any = [{ contains = ["b"] }, { contains = ["c"] }] }]
        "##,
    );
    assert_eq!(detect(&m, &input("a c")).state, State::Blocked);
    assert_eq!(detect(&m, &input("a")).state, State::Unknown);
}

// --- priority and ties ----------------------------------------------------

#[test]
fn the_highest_priority_wins_and_a_tie_keeps_the_first_rule() {
    let m = compile(
        r##"
        id = "t"
        [[rules]]
        id = "first"
        state = "idle"
        priority = 10
        contains = ["x"]
        [[rules]]
        id = "winner"
        state = "working"
        priority = 20
        contains = ["x"]
        [[rules]]
        id = "second"
        state = "blocked"
        priority = 20
        contains = ["x"]
        "##,
    );
    let d = detect(&m, &input("x"));
    assert_eq!(d.state, State::Working);
    assert_eq!(d.rule.as_deref(), Some("winner"), "the first 20-priority rule wins the tie");
}

// --- regions ---------------------------------------------------------------

const SCREEN: &str = "top line\nsecond\nthird\n\nbottom one\nlast line";

#[test]
fn regions_slice_the_screen_as_named() {
    use manifest::Region::*;
    let cases: &[(manifest::Region, &str)] = &[
        (WholeRecent, SCREEN),
        (BottomLines(2), "bottom one\nlast line"),
        (BottomNonEmptyLines(2), "bottom one\nlast line"),
        (BottomNonEmptyLines(9), SCREEN),
        (TopNonEmptyLines(2), "top line\nsecond\n"),
        (BottomLines(0), ""),
    ];
    for (region, want) in cases {
        let got = region.extract(&input(SCREEN));
        assert_eq!(got, *want, "{region:?}");
    }
}

#[test]
fn prompt_box_regions_find_the_composer() {
    let screen = "log line\n────\n❯ typed\n────\n footer";
    let i = input(screen);
    assert_eq!(manifest::Region::PromptBoxBody.extract(&i), "❯ typed\n");
    assert_eq!(manifest::Region::AbovePromptBox.extract(&i), "log line\n");
    assert_eq!(manifest::Region::LastNonEmptyAbovePromptBox.extract(&i), "log line");
    assert_eq!(manifest::Region::AfterLastHorizontalRule.extract(&i), " footer");
    assert_eq!(manifest::Region::PromptBoxBody.extract(&input("no box")), "");
    assert_eq!(manifest::Region::AbovePromptBox.extract(&input("no box")), "no box");
}

#[test]
fn codex_marker_regions_track_the_live_prompt() {
    let screen = "old output\n• done step\n› waiting\n";
    let i = input(screen);
    assert_eq!(manifest::Region::BeforeCurrentPromptMarker.extract(&i), "old output\n• done step\n");
    assert_eq!(manifest::Region::WholeRecentWithoutCurrentPromptMarker.extract(&i), "");
    assert_eq!(manifest::Region::CurrentPromptBlockMarker.extract(&i), "• done step");
    assert_eq!(manifest::Region::AfterCurrentPromptBlockMarker.extract(&i), "• done step\n› waiting\n");
    // A block marker below the prompt retires it.
    let stale = "old\n› answered\n• new work\n";
    let j = input(stale);
    assert_eq!(manifest::Region::WholeRecentWithoutCurrentPromptMarker.extract(&j), stale);
    assert_eq!(manifest::Region::CurrentPromptBlockMarker.extract(&j), "");
}

#[test]
fn osc_regions_read_their_own_fields() {
    let i = Input { screen: "screen text", title: "✳ title", progress: "9;1" };
    assert_eq!(manifest::Region::OscTitle.extract(&i), "✳ title");
    assert_eq!(manifest::Region::OscProgress.extract(&i), "9;1");
}

// --- results ---------------------------------------------------------------

#[test]
fn no_match_is_unknown_and_never_idle() {
    let m = compile(
        r##"
        id = "t"
        [[rules]]
        id = "r"
        state = "idle"
        contains = ["impossible"]
        "##,
    );
    let d = detect(&m, &input("anything"));
    assert_eq!(d.state, State::Unknown);
    assert_eq!(d, NONE);
}

#[test]
fn oversized_input_is_unknown_not_a_match() {
    let m = compile(
        r##"
        id = "t"
        [[rules]]
        id = "r"
        state = "idle"
        contains = ["x"]
        "##,
    );
    let huge = "x".repeat(MAX_INPUT_BYTES + 1);
    assert_eq!(detect(&m, &input(&huge)).state, State::Unknown);
}

#[test]
fn skip_state_update_is_neutral_news() {
    let m = compile(
        r##"
        id = "t"
        [[rules]]
        id = "viewer"
        state = "unknown"
        skip_state_update = true
        contains = ["transcript"]
        "##,
    );
    let d = detect(&m, &input("showing transcript"));
    assert_eq!(d.state, State::Unknown);
    assert!(d.skip, "the caller must leave the inferred status alone");
    assert_eq!(d.rule.as_deref(), Some("viewer"));
}

#[test]
fn blocked_rules_may_carry_a_kind() {
    let m = compile(
        r##"
        id = "t"
        [[rules]]
        id = "perm"
        state = "blocked"
        kind = "permission"
        visible_blocker = true
        contains = ["approve"]
        "##,
    );
    let d = detect(&m, &input("approve?"));
    assert_eq!(d.state, State::Blocked);
    assert_eq!(d.kind, Some("permission"));
    assert!(d.visible_blocker);
}

// --- validation ------------------------------------------------------------

#[test]
fn validation_rejects_bad_manifests() {
    let bad = [
        ("id = \"\"", "empty id"),
        ("id=\"t\"", "no rules"),
        ("id=\"t\"\nmin_engine_version=99\n[[rules]]\nid=\"r\"\ncontains=[\"x\"]", "engine too new"),
        ("id=\"t\"\n[[rules]]\nid=\"\"\ncontains=[\"x\"]", "empty rule id"),
        ("id=\"t\"\n[[rules]]\nid=\"r\"\nstate=\"bogus\"\ncontains=[\"x\"]", "bad state"),
        (
            "id=\"t\"\n[[rules]]\nid=\"r\"\nstate=\"idle\"\nkind=\"permission\"\ncontains=[\"x\"]",
            "kind only on blocked",
        ),
        ("id=\"t\"\n[[rules]]\nid=\"r\"\nstate=\"blocked\"\nkind=\"bogus\"\ncontains=[\"x\"]", "bad kind"),
        (
            "id=\"t\"\n[[rules]]\nid=\"r\"\nskip_state_update=true\nstate=\"idle\"\ncontains=[\"x\"]",
            "skip needs unknown",
        ),
        ("id=\"t\"\n[[rules]]\nid=\"r\"\nregion=\"nonsense\"\ncontains=[\"x\"]", "bad region"),
        ("id=\"t\"\n[[rules]]\nid=\"r\"\nregex=[\"([\"]", "bad regex"),
        ("id=\"t\"\n[[rules]]\nid=\"r\"\nnot=[{}]\ncontains=[\"x\"]", "empty not gate"),
        ("id=\"t\"\n[[rules]]\nid=\"r\"\nmisspelled=[\"x\"]\ncontains=[\"x\"]", "unknown field"),
        ("id=\"t\"\n[[rules]]\nid=\"r\"", "gate needs a matcher"),
    ];
    for (text, why) in bad {
        assert!(Manifest::compile(text).is_err(), "{why}: {text}");
    }
}

#[test]
fn validation_bounds_complexity() {
    let many_rules = format!("id=\"t\"\n{}", "[[rules]]\nid=\"r\"\ncontains=[\"x\"]\n".repeat(129));
    assert!(Manifest::compile(&many_rules).is_err(), "over 128 rules");

    let long = "x".repeat(513);
    let text = format!("id=\"t\"\n[[rules]]\nid=\"r\"\ncontains=[\"{long}\"]");
    assert!(Manifest::compile(&text).is_err(), "over 512 matcher chars");

    let many = (0..33).map(|i| format!("\"m{i}\"")).collect::<Vec<_>>().join(",");
    let text = format!("id=\"t\"\n[[rules]]\nid=\"r\"\ncontains=[{many}]");
    assert!(Manifest::compile(&text).is_err(), "over 32 matchers per gate");

    // Depth: nine nested all gates.
    let mut gate = "contains=[\"x\"]".to_string();
    for _ in 0..9 {
        gate = format!("all=[{{ {gate} }}]");
    }
    let text = format!("id=\"t\"\n[[rules]]\nid=\"r\"\n{gate}");
    assert!(Manifest::compile(&text).is_err(), "over 8 gate depth");
}

// --- bundled manifests -----------------------------------------------------

#[test]
fn bundled_manifests_validate_and_compile() {
    for (id, text) in BUNDLED {
        let m = Manifest::compile(text).unwrap_or_else(|e| panic!("bundled {id}: {e}"));
        assert_eq!(m.id, *id, "{id}: file id must match its key");
        assert!(!m.rules.is_empty());
    }
    assert!(bundled("devin").is_some());
    assert!(bundled("nope").is_none());
}

/// Captured screens and what they must map to. Evidence is in
/// tests/fixtures/detect/; the title is what a real terminal's OSC would
/// carry on that screen (Codex detects mostly from its title).
#[test]
fn captured_screens_map_to_their_states() {
    let cases: &[(&str, &str, &str, State, Option<&str>)] = &[
        // fixture, agent, title, expected state, expected kind
        ("devin-guide-prompt.txt", "devin", "", State::Working, None),
        ("devin-welcome.txt", "devin", "", State::Idle, None),
        ("devin-draft-box.txt", "devin", "", State::Idle, None),
        ("claude-idle.txt", "claude", "", State::Idle, None),
        ("claude-done.txt", "claude", "", State::Idle, None),
        ("claude-working.txt", "claude", "", State::Working, None),
        ("claude-bash-running.txt", "claude", "", State::Idle, None),
        ("claude-approve-bash.txt", "claude", "", State::Blocked, Some("permission")),
        ("codex-idle.txt", "codex", "Codex", State::Idle, None),
        ("codex-approve-command.txt", "codex", "✳ Action Required — Codex", State::Blocked, None),
        ("codex-approve-edits.txt", "codex", "Codex", State::Blocked, Some("permission")),
        ("codex-trust.txt", "codex", "Codex", State::Blocked, Some("permission")),
        ("opencode-permission-edit.txt", "opencode", "", State::Blocked, Some("permission")),
        ("opencode-permission-narrow.txt", "opencode", "", State::Blocked, Some("permission")),
        ("opencode-permission-shell.txt", "opencode", "", State::Blocked, Some("permission")),
        ("cmd-composer.txt", "command-code", "", State::Idle, None),
        ("cmd-draft.txt", "command-code", "", State::Idle, None),
        ("cmd-slash-menu.txt", "command-code", "", State::Idle, None),
        ("cmd-trust-dialog.txt", "command-code", "", State::Blocked, Some("permission")),
    ];
    for (file, agent, title, state, kind) in cases {
        let m = bundled(agent).unwrap();
        let d = detect(&m, &titled(&fixture(file), title));
        assert_eq!(d.state, *state, "{file} on {agent} (rule: {:?})", d.rule);
        assert_eq!(d.kind, *kind, "{file} on {agent} kind (rule: {:?})", d.rule);
    }
}
