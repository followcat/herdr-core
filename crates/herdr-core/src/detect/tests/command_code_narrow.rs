use super::*;

fn expect(screen: &str, state: State, kind: Option<&str>) {
    let m = bundled("command-code").unwrap();
    let d = detect(&m, &input(screen));
    assert_eq!((d.state, d.kind), (state, kind), "rule: {:?}", d.rule);
    if state == State::Working {
        let composer = m.rules.iter().find(|r| r.id == "composer_prompt").unwrap();
        assert!(!composer.matches(&input(screen)), "idle cannot match beneath a live footer");
    }
}

fn draft(text: &str) -> String {
    fixture("cmd-draft.txt").replace("fix the flaky test", text)
}

#[test]
fn multiline_footer_and_permission_drafts_stay_idle() {
    for text in [
        "first line\n ○ Harmonizing…  esc to interrupt • 3s\nlast line",
        "first line\n❯ 1. Yes\nenter select · Command Code needs to\nlast line",
    ] {
        expect(&draft(text), State::Idle, None);
    }
}

#[test]
fn a_composer_without_a_shortcuts_hint_is_idle() {
    expect(&draft("first\nlast").replace("  ? for shortcuts\n", ""), State::Idle, None);
}

#[test]
fn a_retained_footer_then_one_reply_does_not_suppress_idle() {
    let screen = draft("");
    let at = screen.find('─').unwrap();
    expect(&format!(" ○ Harmonizing…  esc to interrupt • 3s\n⠶ DONE\n{}", &screen[at..]), State::Idle, None);
}

#[test]
fn nested_gate_regions_exclude_drafts_and_inherit_within_the_selected_slice() {
    let m = compile(
        r#"
        id = "test"
        [[rules]]
        id = "ready"
        state = "idle"
        region = "prompt_box_body"
        contains = ["❯"]
        not = [{ region = "above_prompt_box", all = [{ contains = ["busy"] }] }]
    "#,
    );
    assert_eq!(detect(&m, &input("ready\n────\n❯ busy\n────\nhint")).state, State::Idle);
    assert_eq!(detect(&m, &input("busy\n────\n❯ ready\n────\nhint")).state, State::Unknown);
    assert!(
        Manifest::compile("id='x'\n[[rules]]\nid='r'\nnot=[{region='bogus',contains=['x']}]\ncontains=['y']").is_err()
    );
}

#[test]
fn a_short_pasted_divider_cannot_expose_a_fake_working_footer() {
    expect(&draft("first\n ○ Harmonizing…  esc to interrupt • 3s\n──────────\nmore text"), State::Idle, None);
}

#[test]
fn full_width_regions_keep_short_dividers_inside_the_composer() {
    for newline in ["\n", "\r\n"] {
        let bar = "─".repeat(32);
        let above = "reply";
        let body = "❯ first\n──────────\n · Fake…  12\nlast".replace('\n', newline);
        let screen = format!("{above}{newline}{bar}{newline}{body}{newline}{bar}{newline}hint");
        let i = input(&screen);
        let region = |name: &str| name.parse::<manifest::Region>().unwrap().extract(&i);
        assert_eq!(region("above_full_width_prompt_box"), format!("{above}{newline}"));
        assert_eq!(region("full_width_prompt_box_body"), format!("{body}{newline}"));
        assert_eq!(region("after_last_full_width_rule"), "hint");
    }
}

macro_rules! capture {
    ($test:ident, $file:literal, $state:ident, $kind:expr) => {
        #[test]
        fn $test() {
            expect(&fixture(concat!("cmd-narrow/", $file, ".txt")), State::$state, $kind);
        }
    };
}

capture!(capture_draft60, "draft60", Idle, None);

#[test]
fn wrapped_full_footers_and_tip_continuations_are_working() {
    let compact = fixture("cmd-narrow/work32-01.txt");
    for replacement in [
        "✻ Worked for 6s",
        "✻ Thought for 6s",
        "⠶ Formulating…",
        " ○ lowercase…",
        " ☆ Formulating…\nnot a footer continuation",
    ] {
        expect(&compact.replace(" ☆ Formulating…", replacement), State::Idle, None);
    }
    let screen = fixture("cmd-after-yes-5.txt");
    let footer = " ○ Pondering…  esc to interrupt • 2m 55s • ↓ 78";
    for wrapped in [
        " ○ Pondering…\n  esc to interrupt • 2m 55s • ↓ 78",
        " ○ Pondering…  esc to interrupt\n  • 2m 55s • ↓ 78",
        " ○ Pondering…  esc to interrupt • 2m 55s • ↓\n  78",
    ] {
        expect(&screen.replace(footer, wrapped), State::Working, None);
    }
    expect(
        &screen.replace("Tip: Use shift+tab to enable accept edits", "Tip: Use shift+tab to\n  enable accept edits"),
        State::Working,
        None,
    );
}

capture!(capture_w60_tip, "w60-04", Working, None);
capture!(capture_work32, "work32-01", Working, None);
capture!(capture_done32, "work32-04", Idle, None);
capture!(capture_work45, "work45-01", Working, None);
capture!(capture_done45, "work45-03", Idle, None);
capture!(capture_work60, "work60-01", Working, None);
capture!(capture_work66, "work66-04", Working, None);
capture!(capture_work72, "work72-01", Working, None);
capture!(capture_work80, "work80-01", Working, None);
capture!(capture_done80, "work80-17", Idle, None);

#[test]
fn a_dialog_rewrapped_at_25_columns_is_permission_blocked() {
    let source = fixture("cmd-tool-permission.txt");
    let (_, dialog) = source.rsplit_once('─').unwrap();
    // Word wrapping deliberately splits both phrases across rows. The
    // original command/choices/control text is retained.
    let mut rows = Vec::new();
    for line in dialog.lines() {
        let mut row = String::new();
        for word in line.split_whitespace() {
            if !row.is_empty() && row.chars().count() + 1 + word.chars().count() > 25 {
                rows.push(std::mem::take(&mut row));
            }
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(word);
        }
        rows.push(row);
    }
    // 25 columns split the header only when aligned after its leading
    // prefix; force the phrase boundary the reviewer also reproduced.
    let wrapped = rows
        .join("\n")
        .replace("Command Code needs to", "Command Code needs\nto")
        .replace("enter select", "enter\nselect");
    expect(&format!("{}\n{wrapped}", "─".repeat(25)), State::Blocked, Some("permission"));
    expect(&source.replace("3. No,", "3. Nope,"), State::Unknown, None);
}

capture!(capture_w32_dialog, "w32-02-dialog", Blocked, Some("permission"));
capture!(capture_w45_dialog, "w45-10-dialog", Blocked, Some("permission"));
capture!(capture_w60_dialog, "w60-02-dialog", Blocked, Some("permission"));
