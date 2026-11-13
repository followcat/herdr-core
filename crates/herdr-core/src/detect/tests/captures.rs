use super::*;

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
        ("claude-bash-starting.txt", "claude", "", State::Working, None),
        ("claude-bash-running.txt", "claude", "", State::Working, None),
        ("claude-bash-finished.txt", "claude", "", State::Idle, None),
        ("claude-approve-bash.txt", "claude", "", State::Blocked, Some("permission")),
        ("codex-idle.txt", "codex", "Codex", State::Unknown, None),
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
        ("cmd-resume-picker.txt", "command-code", "", State::Blocked, Some("question")),
        ("cursor-perm-shell.txt", "cursor", "", State::Blocked, Some("permission")),
        ("cursor-write-file.txt", "cursor", "", State::Blocked, Some("permission")),
        ("cursor-working.txt", "cursor", "", State::Working, None),
        ("cursor-idle.txt", "cursor", "", State::Idle, None),
        ("cursor-trust.txt", "cursor", "", State::Blocked, Some("permission")),
        ("cursor-resume-picker.txt", "cursor", "", State::Blocked, Some("question")),
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

/// A `!` block's live controls only count inside the current (last) block:
/// retained `ctrl+b`/timer text from an older, finished shell must not keep
/// the screen working. Composed from captured lines, not invented text.
#[test]
fn retained_shell_controls_from_an_older_block_are_not_working() {
    fn block<'s>(screen: &'s str, from: &str, to: &str) -> &'s str {
        let start = screen.find(from).unwrap();
        let end = screen[start..].find(to).map(|i| start + i + to.len()).unwrap();
        &screen[start..end]
    }
    let running = fixture("claude-bash-running.txt");
    let finished = fixture("claude-bash-finished.txt");
    let composer = &finished[finished.find("───").unwrap()..];
    let stale = format!(
        "{}\n{}\n{}",
        block(&running, "! ", "(ctrl+b to run in background)"), // retained controls
        block(&finished, "! ", "… +27 lines"),                  // the current, finished block
        composer,
    );
    let m = bundled("claude").unwrap();
    let i = input(&stale);
    assert_eq!(manifest::Region::CurrentShellBlock.extract(&i), format!("{}\n", block(&finished, "! ", "… +27 lines")));
    let d = detect(&m, &i);
    assert_eq!(d.state, State::Idle, "stale controls must not win (rule: {:?})", d.rule);
}

/// A live `!` block is only working while its controls are the last
/// transcript content above the composer: a `⏺` reply, a `✻ … done` line or
/// any tool output after them means the block finished. Blank lines, queued
/// input and the block's own timer-bearing `+N lines (Ns)` progress line are
/// the only allowed tail. Composed from captured lines, not invented text.
#[test]
fn shell_controls_must_be_the_last_transcript_content() {
    fn block<'s>(screen: &'s str, from: &str, to: &str) -> &'s str {
        let start = screen.find(from).unwrap();
        let end = screen[start..].find(to).map(|i| start + i + to.len()).unwrap();
        &screen[start..end]
    }
    let running = fixture("claude-bash-running.txt");
    let finished = fixture("claude-bash-finished.txt");
    let done = fixture("claude-done.txt");
    let composer = &finished[finished.find("───").unwrap()..];
    // Captured lines of the running block: `!`, steps, `+3 lines (7s)`,
    // `(ctrl+b to run in background)`.
    let shell: Vec<&str> = block(&running, "! ", "(ctrl+b to run in background)").lines().collect();
    let (progress, controls) = (shell[shell.len() - 2], shell[shell.len() - 1]);
    let m = bundled("claude").unwrap();

    // The untouched captures still hold: live controls work, a collapsed
    // `… +N lines` block without them is idle.
    assert_eq!(detect(&m, &input(&running)).state, State::Working);
    assert_eq!(detect(&m, &input(&finished)).state, State::Idle);

    // The captured claude-done screen appended after the running block's
    // controls: the `⏺` reply and `✻ … done` are newer transcript content.
    let finished_later = format!("{}\n{}", block(&running, "! ", "(ctrl+b to run in background)"), done);
    let d = detect(&m, &input(&finished_later));
    assert_eq!(d.state, State::Idle, "newer transcript defeats the controls (rule: {:?})", d.rule);

    // The same collapse marker after the controls: `… +N lines` has no
    // `(Ns)` timer, so it is finished evidence, not live progress.
    let collapsed = format!("{}\n{}\n     … +27 lines\n\n{}", shell[0], controls, composer);
    let d = detect(&m, &input(&collapsed));
    assert_eq!(d.state, State::Idle, "a collapse tail defeats the controls (rule: {:?})", d.rule);

    // The timer-bearing `+3 lines (7s)` progress line — whether it sits
    // before the controls or after them — is live evidence, not a collapse.
    for (name, tail_progress) in [("progress before controls", false), ("progress after controls", true)] {
        let live = if tail_progress {
            format!("{}\n{}\n{}\n\n{}", shell[0], controls, progress, composer)
        } else {
            format!("{}\n{}\n{}\n\n{}", shell[0], progress, controls, composer)
        };
        let d = detect(&m, &input(&live));
        assert_eq!(d.state, State::Working, "{name} (rule: {:?})", d.rule);
    }

    // The progress alternative spans a whole line: text before or after the
    // timer substring is finished evidence, not live progress.
    let marker = done.lines().next().unwrap().chars().next().unwrap(); // '⏺'
    let finished_reply =
        format!("{}\n{}\n     {marker} Finished: {}\n\n{}", shell[0], controls, progress.trim(), composer);
    let d = detect(&m, &input(&finished_reply));
    assert_eq!(d.state, State::Idle, "a ⏺ reply quoting the timer defeats the controls (rule: {:?})", d.rule);
    let done_tail = finished.lines().next().unwrap().split_once('·').unwrap().1.trim(); // "done 10:02 am"
    let collapsed_done = format!("{}\n{}\n     … +27 lines (7s) · {done_tail}\n\n{}", shell[0], controls, composer);
    let d = detect(&m, &input(&collapsed_done));
    assert_eq!(d.state, State::Idle, "a collapse tail with a timer still defeats the controls (rule: {:?})", d.rule);

    // A `!` draft inside the composer is input, not a shell block: the
    // detection is unchanged.
    let drafted = finished.replacen("\n❯\n", "\n❯ ! ls -la\n", 1);
    let d = detect(&m, &input(&drafted));
    assert_eq!(d.state, State::Idle, "composer draft changes nothing (rule: {:?})", d.rule);
}

/// A Codex title names who is running — it is not readiness evidence. An
/// empty or non-prompt screen stays Unknown until captured readiness
/// evidence exists; question controls get `question`, permission controls
/// `permission`.
#[test]
fn codex_title_alone_is_not_readiness_and_kinds_split() {
    let m = bundled("codex").unwrap();
    assert_eq!(detect(&m, &titled("", "Codex")).state, State::Unknown, "an empty screen titled Codex is not ready");
    let idle = fixture("codex-idle.txt");
    assert_eq!(detect(&m, &titled(&idle, "Codex")).state, State::Unknown, "a Codex title alone is not readiness");
    assert_eq!(detect(&m, &titled(&idle, "")).state, State::Unknown);
    let foreign = "$ cargo build\n   Compiling demo v0.1.0\n";
    assert_eq!(
        detect(&m, &titled(foreign, "Codex")).state,
        State::Unknown,
        "a stale Codex title on another screen fails closed"
    );

    let ask = "›\n enter to submit answer\n";
    assert_eq!(detect(&m, &input(ask)).kind, Some("question"), "submit answer is a question");
    let ask_all = "›\n enter to submit all\n";
    assert_eq!(detect(&m, &input(ask_all)).kind, Some("question"), "submit all is a question");
    let perm = "›\n press enter to confirm or esc to cancel\n";
    assert_eq!(detect(&m, &input(perm)).kind, Some("permission"), "confirm/cancel is a permission");
}

fn cmd_capture(file: &str, state: State, kind: Option<&str>) {
    let m = bundled("command-code").unwrap();
    let d = detect(&m, &input(&fixture(file)));
    assert_eq!((d.state, d.kind), (state, kind), "{file} (rule: {:?})", d.rule);
}

#[test]
fn cmd_harmonizing_turn_is_working() {
    cmd_capture("cmd2-turn-1.txt", State::Working, None);
}

#[test]
fn cmd_pondering_with_a_tip_is_working() {
    cmd_capture("cmd-after-yes-5.txt", State::Working, None);
}

#[test]
fn cmd_allowed_shell_footer_is_working() {
    cmd_capture("cmd2-run-8.txt", State::Working, None);
}

#[test]
fn cmd_shell_permission_is_blocked() {
    cmd_capture("cmd-tool-permission.txt", State::Blocked, Some("permission"));
}

#[test]
fn cmd_later_turn_permission_is_blocked() {
    cmd_capture("cmd2-turn-11.txt", State::Blocked, Some("permission"));
}

#[test]
fn cmd_retry_permission_is_blocked() {
    // The brief calls this a working footer, but the supplied screen is a
    // second permission dialog. cmd2-run-8 supplies the actual footer.
    cmd_capture("cmd2-run-10.txt", State::Blocked, Some("permission"));
}

#[test]
fn cmd_done_without_a_turn_footer_is_idle() {
    cmd_capture("cmd-after-yes-15.txt", State::Idle, None);
}

#[test]
fn cmd_draft_controls_do_not_claim_work_or_permission() {
    let m = bundled("command-code").unwrap();
    let draft = fixture("cmd-draft.txt");
    for text in [
        "explain esc to interrupt • 3",
        "enter select",
        "Command Code needs to execute enter select",
        "○ Harmonizing…  esc to interrupt • 3s • ↓ 0",
    ] {
        let screen = draft.replace("fix the flaky test", text);
        let d = detect(&m, &input(&screen));
        assert_eq!((d.state, d.kind), (State::Idle, None), "draft {text:?} (rule: {:?})", d.rule);
    }
}

#[test]
fn cmd_composer_cannot_claim_busy_or_permission_screens() {
    let m = bundled("command-code").unwrap();
    let composer = m.rules.iter().find(|r| r.id == "composer_prompt").unwrap();
    for file in [
        "cmd2-turn-1.txt",
        "cmd-after-yes-5.txt",
        "cmd2-run-8.txt",
        "cmd-tool-permission.txt",
        "cmd2-turn-11.txt",
        "cmd2-run-10.txt",
    ] {
        assert!(!composer.matches(&input(&fixture(file))), "composer still matches {file}");
    }
}

#[test]
fn cmd_permission_requires_the_dialog_and_choices_not_just_controls() {
    let m = bundled("command-code").unwrap();
    let screen = fixture("cmd-tool-permission.txt");
    for incomplete in [
        screen.replace("Command Code needs to execute touch cmd-was-here.txt.", ""),
        screen.replace("❯ 1. Yes", ""),
        screen.replace("enter select", ""),
    ] {
        let d = detect(&m, &input(&incomplete));
        assert_eq!(d.state, State::Unknown, "incomplete permission evidence (rule: {:?})", d.rule);
    }
}

#[test]
fn cmd_retained_footer_above_a_finished_reply_is_idle() {
    let working = fixture("cmd2-turn-1.txt");
    let footer = working.lines().find(|l| l.contains("esc to interrupt")).unwrap();
    let finished = fixture("cmd-after-yes-15.txt");
    let screen = finished.replace("⠶ DONE", &format!("{footer}\n⠶ DONE"));
    let m = bundled("command-code").unwrap();
    let d = detect(&m, &input(&screen));
    assert_eq!((d.state, d.kind), (State::Idle, None), "retained footer (rule: {:?})", d.rule);
}
