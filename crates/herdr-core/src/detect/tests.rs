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
        ("codex-approve-command.txt", "codex", "Codex", State::Blocked, Some("permission")),
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

/// The command-approval screen must be blocked from screen evidence alone:
/// its `› N.` options are a selection cursor, so no "Action Required" title
/// is allowed to hide them — and idle must not claim it either.
#[test]
fn codex_command_approval_is_blocked_under_any_title() {
    let m = bundled("codex").unwrap();
    let screen = fixture("codex-approve-command.txt");
    for title in ["", "bash", "Codex"] {
        let d = detect(&m, &titled(&screen, title));
        assert_eq!(d.state, State::Blocked, "title {title:?} (rule: {:?})", d.rule);
        assert_eq!(d.kind, Some("permission"), "title {title:?} (rule: {:?})", d.rule);
    }
}

/// Codex idle needs positive title evidence naming Codex; question controls
/// get `question`, permission controls `permission`.
#[test]
fn codex_idle_needs_a_codex_title_and_kinds_split() {
    let m = bundled("codex").unwrap();
    let idle = fixture("codex-idle.txt");
    assert_eq!(detect(&m, &titled(&idle, "")).state, State::Unknown, "absent title is not idle evidence");
    assert_eq!(detect(&m, &titled(&idle, "bash")).state, State::Unknown, "stale title is not idle evidence");
    assert_eq!(detect(&m, &titled(&idle, "Codex")).state, State::Idle);

    let ask = "›\n enter to submit answer\n";
    assert_eq!(detect(&m, &input(ask)).kind, Some("question"), "submit answer is a question");
    let ask_all = "›\n enter to submit all\n";
    assert_eq!(detect(&m, &input(ask_all)).kind, Some("question"), "submit all is a question");
    let perm = "›\n press enter to confirm or esc to cancel\n";
    assert_eq!(detect(&m, &input(perm)).kind, Some("permission"), "confirm/cancel is a permission");
}

// --- foreground identification ----------------------------------------------

use crate::foreground::Foreground;
use catalog::{Catalog, Limits};

fn fg(name: &str, argv: &[&str]) -> Foreground {
    Foreground { pid: 1, name: name.into(), argv: argv.iter().map(|s| s.to_string()).collect(), cwd: "/tmp".into() }
}

#[test]
fn identifies_native_executables_paths_and_suffixes() {
    let cases: &[(&str, &[&str], Option<&str>)] = &[
        ("claude", &["claude"], Some("claude")),
        ("codex", &["codex", "--ask"], Some("codex")),
        ("devin", &["devin"], Some("devin")),
        ("opencode", &["opencode"], Some("opencode")),
        ("claude-code", &["claude-code"], Some("claude")),
        ("devin-cli", &["devin-cli"], Some("devin")),
        ("opencode2", &["opencode2"], Some("opencode")),
        // Paths: the basename, not the folders, carries the name.
        ("node", &["/usr/local/bin/codex", "resume"], Some("codex")),
        ("cmd", &["/home/user/.local/bin/opencode", "run"], Some("opencode")),
        // Windows-style suffixes and separators.
        ("claude.exe", &["claude.exe"], Some("claude")),
        ("node", &["C:\\tools\\claude.CMD"], Some("claude")),
        ("codex.ps1", &["codex.ps1"], Some("codex")),
        ("node", &["/opt/devin/devin.js"], Some("devin")),
        // Uppercased names still count.
        ("CLAUDE", &["CLAUDE"], Some("claude")),
    ];
    for (name, argv, want) in cases {
        assert_eq!(identify(&fg(name, argv)), *want, "{name} {argv:?}");
    }
}

#[test]
fn identifies_interpreter_entrypoints_not_mentions() {
    let cases: &[(&str, &[&str], Option<&str>)] = &[
        // node script position: path basename or package dir is the evidence.
        ("node", &["node", "/home/user/.claude/local/cli.js"], None),
        ("node", &["node", "/opt/devin/devin.js", "--version"], Some("devin")),
        ("node", &["node", "/usr/lib/node_modules/command-code/dist/cli.js"], Some("command-code")),
        ("node", &["node", "/usr/lib/node_modules/@openai/codex/bin/codex.js"], Some("codex")),
        ("node", &["node", "/usr/lib/node_modules/@anthropic-ai/claude-code/cli.js"], Some("claude")),
        ("node", &["node", "/usr/lib/node_modules/opencode-ai/dist/index.js"], Some("opencode")),
        // Runtime flags are skipped; the script stays the entrypoint.
        ("node", &["node", "--inspect", "/opt/codex/codex.js"], Some("codex")),
        ("node", &["node", "--require", "tsm", "/opt/claude/claude.js"], Some("claude")),
        ("node", &["node", "--", "/opt/devin/devin.js"], Some("devin")),
        // `node -e …` and `sh -c …` are inline source: nothing to inspect.
        ("node", &["node", "-e", "require('devin')"], None),
        ("sh", &["sh", "-c", "devin --version"], None),
        // python -m uses the module's last dotted component.
        ("python3", &["python3", "-m", "devin_cli"], Some("devin")),
        ("python", &["python", "-m", "codex"], Some("codex")),
        ("python3.12", &["python3.12", "-m", "opencode"], Some("opencode")),
        ("python", &["python", "-c", "import devin"], None),
        // Mentions in later arguments never classify.
        ("vim", &["vim", "devin-notes.md"], None),
        ("node", &["node", "/home/user/devin/main.js"], None),
        ("less", &["less", "claude-transcript.txt"], None),
        ("python3", &["python3", "-m", "venv", "codex-env"], None),
        // Plain shells and the OS cmd stay unidentified.
        ("bash", &["bash"], None),
        ("sh", &["sh", "-l"], None),
        ("zsh", &["zsh"], None),
        ("cmd", &["cmd"], None),
        ("cmd.exe", &["cmd.exe", "/c", "ver"], None),
        ("powershell", &["powershell", "-Command", "claude"], None),
        // A bare `cmd` word is never Command Code on its own.
        ("cmd", &["cmd", "chat"], None),
    ];
    for (name, argv, want) in cases {
        assert_eq!(identify(&fg(name, argv)), *want, "{name} {argv:?}");
    }
}

#[test]
fn command_code_needs_package_evidence_never_a_bare_cmd() {
    // The npm `command-code` package's Unix binary is `cmd`; only a resolved
    // path through the package may identify it.
    let shim = std::env::temp_dir().join(format!("hc-detect-cmd-{}", std::process::id()));
    let pkg = shim.join("node_modules/command-code/dist");
    std::fs::create_dir_all(&pkg).unwrap();
    let entry = pkg.join("cli.js");
    std::fs::write(&entry, "// stub").unwrap();
    let link = shim.join("cmd");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&entry, &link).unwrap();
    #[cfg(windows)]
    std::fs::copy(&entry, &link).unwrap();

    let link_str = link.to_str().unwrap().to_string();
    assert_eq!(identify(&fg("cmd", &[&link_str])), Some("command-code"), "resolved shim identifies the package");
    assert_eq!(
        identify(&fg("node", &["node", entry.to_str().unwrap()])),
        Some("command-code"),
        "script path in the package"
    );
    std::fs::remove_dir_all(&shim).unwrap();
}

// --- the manifest catalog ----------------------------------------------------

fn overrides(tag: &str, entries: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("hc-detect-catalog-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, text) in entries {
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir
}

const ALT_CLAUDE: &str = r#"
id = "claude"
[[rules]]
id = "custom_marker"
state = "working"
contains = ["my custom marker"]
"#;

#[test]
fn a_valid_override_replaces_the_bundled_manifest() {
    let dir = overrides("valid", &[("claude.toml", ALT_CLAUDE)]);
    let cat = Catalog::load(Some(&dir), None);
    let m = cat.get("claude").unwrap();
    assert_eq!(m.rules.len(), 1, "override wins over bundled");
    let d = detect(&m, &input("my custom marker"));
    assert_eq!(d.state, State::Working);
    assert!(cat.warnings.is_empty(), "{:?}", cat.warnings);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_bad_override_falls_back_to_bundled_with_a_warning() {
    let dir =
        overrides("bad", &[("claude.toml", "id = \"claude\"\nnot toml at all [[[\n"), ("bogus.toml", ALT_CLAUDE)]);
    let cat = Catalog::load(Some(&dir), None);
    assert!(cat.get("claude").unwrap().rules.len() > 1, "bundled rules kept");
    assert_eq!(cat.warnings.len(), 2, "{:?}", cat.warnings);
    assert!(cat.warnings.iter().any(|w| w.contains("bogus.toml")), "{:?}", cat.warnings);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_override_must_name_the_agent_its_file_says() {
    let dir = overrides("mismatch", &[("devin.toml", "id = \"codex\"\n[[rules]]\nid=\"r\"\ncontains=[\"x\"]\n")]);
    let cat = Catalog::load(Some(&dir), None);
    assert!(cat.get("devin").unwrap().rules.len() > 1, "bundled devin kept");
    assert_eq!(cat.warnings.len(), 1, "{:?}", cat.warnings);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn disabled_and_limited_config_shape_the_catalog() {
    let off = Catalog::load(None, Some(&Limits { enabled: Some(false), agents: None }));
    assert!(off.get("claude").is_none() && off.get("devin").is_none(), "disabled means empty");

    let only = Catalog::load(None, Some(&Limits { enabled: None, agents: Some(vec!["claude".into(), "nope".into()]) }));
    assert!(only.get("claude").is_some());
    assert!(only.get("devin").is_none() && only.get("codex").is_none(), "agents limits the set");
    assert_eq!(only.warnings.len(), 1, "unknown agent name warns");
    // Aliases still resolve inside an allowed manifest.
    assert!(only.get("claude-code").is_some(), "alias for an allowed agent");
}

#[test]
fn limits_parse_from_the_config_table() {
    let cfg: crate::config::Config = toml::from_str("[detect]\nenabled = false\nagents = [\"claude\"]\n").unwrap();
    let limits = cfg.detect.unwrap();
    assert_eq!(limits.enabled, Some(false));
    assert_eq!(limits.agents.as_deref(), Some(&["claude".to_string()][..]));
    let cfg: crate::config::Config = toml::from_str("").unwrap();
    assert!(cfg.detect.is_none(), "no table means defaults");
}
