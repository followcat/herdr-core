use super::*;

fn expect(screen: &str, state: State, kind: Option<&str>) {
    let m = bundled("command-code").unwrap();
    let d = detect(&m, &input(screen));
    assert_eq!((d.state, d.kind), (state, kind), "rule: {:?}", d.rule);
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
