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

/// A non-empty negative-only gate is meaningful inside a `not` context;
/// empty gates and the complexity budgets still apply.
#[test]
fn nested_negative_gates_compose() {
    let m = compile(
        r##"
        id = "t"
        [[rules]]
        id = "r"
        state = "idle"
        contains = ["x"]
        not = [{ not = [{ contains = ["safe"] }] }]
        "##,
    );
    // not[not[safe]] is present-or-absent evidence: "safe" must appear.
    assert_eq!(detect(&m, &input("x safe")).state, State::Idle);
    assert_eq!(detect(&m, &input("x")).state, State::Unknown);

    for text in [
        "id=\"t\"\n[[rules]]\nid=\"r\"\ncontains=[\"x\"]\nnot=[{}]",
        "id=\"t\"\n[[rules]]\nid=\"r\"\ncontains=[\"x\"]\nnot=[{not=[{}]}]",
        "id=\"t\"\n[[rules]]\nid=\"r\"\ncontains=[\"x\"]\nall=[{not=[{contains=[\"y\"]}]}]",
    ] {
        assert!(Manifest::compile(text).is_err(), "{text}");
    }
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

/// `lines()` eats `\r\n`, so offsets must come from real newline positions:
/// `len + 1` drifts one byte per line and slices a multi-byte char in half.
#[test]
fn crlf_and_unicode_do_not_drift_region_offsets() {
    let screen = "❯ première ligne\r\n│ résumé ✓\r\n───\r\n❯ en attente";
    let i = input(screen);
    assert_eq!(manifest::Region::AfterLastHorizontalRule.extract(&i), "❯ en attente");
    assert_eq!(manifest::Region::BottomLines(1).extract(&i), "❯ en attente");
    assert_eq!(manifest::Region::TopNonEmptyLines(1).extract(&i), "❯ première ligne\r\n");
    assert_eq!(manifest::Region::BottomNonEmptyLines(2).extract(&i), "───\r\n❯ en attente");

    let boxed = "log é\r\n───\r\n❯ typed ü\r\n───\r\nfooter";
    assert_eq!(manifest::Region::PromptBoxBody.extract(&input(boxed)), "❯ typed ü\r\n");
    assert_eq!(manifest::Region::AbovePromptBox.extract(&input(boxed)), "log é\r\n");
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

mod captures;

// --- foreground identification ----------------------------------------------

use crate::foreground::Foreground;
use catalog::{Catalog, Limits};

fn fg(name: &str, argv: &[&str]) -> Foreground {
    Foreground {
        pid: 1,
        name: name.into(),
        argv: argv.iter().map(|s| s.to_string()).collect(),
        cwd: "/tmp".into(),
        started: None,
        agent: None,
    }
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

/// Options mean different things per interpreter: attached values, real
/// value flags, module flags and permission families are handled — an
/// unknown option or a value that names an agent yields no identification.
#[test]
fn interpreter_options_parse_per_runtime() {
    let cases: &[(&str, &[&str], Option<&str>)] = &[
        // python `-S` takes no value; `-m` may carry its module attached.
        ("python3", &["python3", "-S", "-m", "devin_cli"], Some("devin")),
        ("python3", &["python3", "-mdevin_cli"], Some("devin")),
        ("python3", &["python3", "-X", "dev", "-m", "codex"], Some("codex")),
        ("python3", &["python3", "-Wignore", "-m", "opencode"], Some("opencode")),
        // node booleans accept `=value`; deno needs `run` and allows families.
        ("node", &["node", "--inspect-brk=9229", "/opt/codex/codex.js"], Some("codex")),
        // Node 24 switches: `--entry-url` takes no value; `--no-<x>` passes
        // only as a spelling `node --help` itself lists (node rejects the
        // rest), so documented negations identify but an invented one — or
        // the negation of a boolean node won't negate — fails closed.
        ("node", &["node", "--entry-url", "file:///tmp/app.js", "/opt/codex/codex.js"], None),
        ("node", &["node", "--entry-url", "file:///opt/codex/codex.js"], Some("codex")),
        ("node", &["node", "--no-deprecation", "/opt/codex/codex.js"], Some("codex")),
        ("node", &["node", "--no-warnings", "/opt/codex/codex.js"], Some("codex")),
        ("node", &["node", "--no-experimental-strip-types", "/opt/claude/claude.js"], Some("claude")),
        ("node", &["node", "--no-such-hc-review-option", "/opt/codex/codex.js"], None),
        ("node", &["node", "--no-napi-modules", "/opt/codex/codex.js"], None),
        ("node", &["node", "--no-node-memory-debug", "/opt/codex/codex.js"], None),
        ("node", &["node", "--allow-fs-read=/etc", "/opt/claude/claude.js"], Some("claude")),
        // Non-entrypoint modes: a package script, test patterns or inline
        // source are never the agent's file.
        ("node", &["node", "--run", "build", "/opt/codex/codex.js"], None),
        ("node", &["node", "--test", "/opt/codex/codex.js"], None),
        ("node", &["node", "-e", "1", "/opt/codex/codex.js"], None),
        ("node", &["node", "-c", "/opt/codex/codex.js"], None),
        // V8-inherited flags take `=` only: the separate form is rejected
        // by node, fails closed and never consumes the next argument.
        ("node", &["node", "--max-old-space-size", "512", "/opt/codex/codex.js"], None),
        ("node", &["node", "--max-old-space-size=512", "/opt/codex/codex.js"], Some("codex")),
        ("node", &["node", "--stack-size=984", "/opt/devin/devin-cli.js"], Some("devin")),
        // `--policy` is not a Node 24 option at all.
        ("node", &["node", "--policy", "/opt/codex/codex.js"], None),
        ("deno", &["deno", "run", "--allow-read=/tmp", "/opt/devin/devin-cli.js"], Some("devin")),
        ("deno", &["deno", "/opt/devin/devin-cli.js"], None),
        ("bun", &["bun", "--watch", "/usr/lib/node_modules/opencode-ai/dist/index.js"], Some("opencode")),
        // An option's value is not an entrypoint, even when it names an agent.
        ("node", &["node", "--title", "/opt/codex/codex.js", "/tmp/app.js"], None),
        ("node", &["node", "--icu-data-dir", "/opt/codex", "app.js"], None),
        ("node", &["node", "--require", "/opt/claude/register.js"], None),
        ("python3", &["python3", "-X", "devin", "-m", "venv"], None),
        // An unknown option is no evidence — the scan must not guess.
        ("node", &["node", "--not-a-node-flag", "/opt/codex/codex.js"], None),
        ("python3", &["python3", "--bogus", "-m", "devin_cli"], None),
        ("bash", &["bash", "-l", "/opt/devin/devin.sh"], None),
    ];
    for (name, argv, want) in cases {
        assert_eq!(identify(&fg(name, argv)), *want, "{name} {argv:?}");
    }
}

/// The npm `command-code` package's Unix binary is a `cmd` symlink into the
/// package; only the resolved path may identify it. Windows `cmd.exe` has no
/// such entrypoint, so the shim check runs where symlinks exist.
#[cfg(unix)]
#[test]
fn command_code_needs_package_evidence_never_a_bare_cmd() {
    let shim = std::env::temp_dir().join(format!("hc-detect-cmd-{}", std::process::id()));
    let pkg = shim.join("node_modules/command-code/dist");
    std::fs::create_dir_all(&pkg).unwrap();
    let entry = pkg.join("cli.js");
    std::fs::write(&entry, "// stub").unwrap();
    let link = shim.join("cmd");
    std::os::unix::fs::symlink(&entry, &link).unwrap();

    let link_str = link.to_str().unwrap().to_string();
    assert_eq!(identify(&fg("cmd", &[&link_str])), Some("command-code"), "resolved shim identifies the package");
    assert_eq!(
        identify(&fg("node", &["node", entry.to_str().unwrap()])),
        Some("command-code"),
        "script path in the package"
    );
    std::fs::remove_dir_all(&shim).unwrap();
}

/// A `…/node_modules/command-code/…` entrypoint classifies from the path
/// alone — the Windows package layout needs no shim. A `cmd` outside the
/// package, symlink or not, stays unidentified.
#[test]
fn command_code_identifies_only_its_package_entrypoints() {
    for path in [
        "C:\\Users\\user\\node_modules\\command-code\\dist\\cli.js",
        "C:/Users/user/node_modules/command-code/dist/cli.js",
        "/usr/lib/node_modules/command-code/dist/cli.js",
    ] {
        assert_eq!(identify(&fg("node", &["node", path])), Some("command-code"), "{path}");
    }
    assert_eq!(identify(&fg("cmd", &["C:\\Tools\\cmd"])), None, "a cmd outside the package is not evidence");
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
    // Aliases are metadata, not lookup keys: get resolves canonical ids only.
    assert!(only.get("claude-code").is_none(), "aliases do not resolve through get");
}

/// Overrides are bounded regular files: a `devin.toml` directory or a file
/// past `MAX_MANIFEST_BYTES` must not be read — the bundled manifest stays.
#[test]
fn overrides_are_bounded_regular_files() {
    let tag = format!("hc-detect-catalog-bounds-{}", std::process::id());
    let dir = std::env::temp_dir().join(&tag);
    std::fs::create_dir_all(dir.join("codex.toml")).unwrap();
    let max = catalog::MAX_MANIFEST_BYTES as usize;
    std::fs::write(dir.join("devin.toml"), vec![b'#'; max + 1]).unwrap();
    // Exactly at the bound still loads: a small manifest padded with comments.
    // ALT_CLAUDE ends with a newline, so the padding stays a comment line.
    let mut at = ALT_CLAUDE.replace("id = \"claude\"", "id = \"opencode\"").into_bytes();
    at.resize(max, b'#');
    std::fs::write(dir.join("opencode.toml"), at).unwrap();

    let cat = Catalog::load(Some(&dir), None);
    assert!(cat.get("codex").unwrap().rules.len() > 1, "directory keeps bundled codex");
    assert!(cat.get("devin").unwrap().rules.len() > 1, "oversized keeps bundled devin");
    assert_eq!(cat.get("opencode").unwrap().rules.len(), 1, "manifest at the bound loads");
    assert_eq!(cat.warnings.len(), 2, "{:?}", cat.warnings);
    assert!(cat.warnings.iter().any(|w| w.contains("regular file")), "{:?}", cat.warnings);
    assert!(cat.warnings.iter().any(|w| w.contains("bytes")), "{:?}", cat.warnings);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A user override's declared aliases must not shadow another agent or skip
/// the allowlist — `get` resolves canonical ids exactly.
#[test]
fn an_override_alias_cannot_hijack_another_agent() {
    let hijack = "id = \"claude\"\naliases = [\"devin\"]\n[[rules]]\nid = \"r\"\ncontains = [\"x\"]\n";
    let dir = overrides("hijack", &[("claude.toml", hijack)]);

    let only_claude = Catalog::load(Some(&dir), Some(&Limits { enabled: None, agents: Some(vec!["claude".into()]) }));
    assert!(only_claude.get("claude").is_some());
    assert!(only_claude.get("devin").is_none(), "an alias cannot bypass the allowlist");

    let both = Catalog::load(Some(&dir), None);
    let devin = both.get("devin").expect("bundled devin stays reachable");
    assert_eq!(devin.id, "devin", "claude's alias must not shadow devin");
    assert_ne!(devin.rules.len(), 1, "devin is its bundled manifest, not the claude override");
    std::fs::remove_dir_all(&dir).unwrap();
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

mod command_code_narrow;
