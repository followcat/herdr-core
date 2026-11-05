use super::*;

#[test]
fn soft_wraps_join_but_identical_hard_rows_and_drafts_do_not() {
    let m = bundled("command-code").unwrap();
    let mut soft = ghostty_vt::Terminal::new(16, 16, 0);
    let mut hard = ghostty_vt::Terminal::new(16, 16, 0);
    let label = " ○ Conceptualizing…";
    let cut = label.char_indices().nth(16).unwrap().0;
    let bar = "─".repeat(16);
    let box_text = format!("\r\n{bar}\r\n❯ Ask\r\n{bar}\r\n? for shortcuts");
    soft.write(format!("{label}{box_text}").as_bytes());
    hard.write(format!("{}\r\n{}{box_text}", &label[..cut], &label[cut..]).as_bytes());
    assert_eq!(soft.screen_rows(), hard.screen_rows(), "only the VT's wrap facts differ");
    for (vt, expected) in [(&soft, State::Working), (&hard, State::Idle)] {
        let rows = vt.screen_rows();
        let logical = vt.screen_logical_text();
        let verdict = crate::screen_status::detect_rows(&m, &rows, "", Some(&logical));
        assert_eq!(verdict.state, expected, "{logical}");
        let physical = rows.join("\n");
        let idle = m.rules.iter().find(|r| r.id == "composer_prompt").unwrap();
        assert_eq!(idle.matches_with_logical_screen(&input(&physical), Some(&logical)), expected == State::Idle);
    }
    let raw = compile(
        &BUNDLED
            .iter()
            .find(|(id, _)| *id == "command-code")
            .unwrap()
            .1
            .replace("logical_lines = true", "logical_lines = false"),
    );
    assert!(!raw.needs_logical_lines());
    assert_eq!(
        crate::screen_status::detect_rows(&raw, &soft.screen_rows(), "", Some(&soft.screen_logical_text())).state,
        State::Idle
    );
    for text in
        [format!("  ○ Conceptualizing…{box_text}"), format!("{bar}\r\n❯ draft\r\n{label}\r\n{bar}\r\n? for shortcuts")]
    {
        let mut vt = ghostty_vt::Terminal::new(16, 16, 0);
        vt.write(text.as_bytes());
        assert_eq!(
            crate::screen_status::detect_rows(&m, &vt.screen_rows(), "", Some(&vt.screen_logical_text())).state,
            State::Idle
        );
    }
}

#[test]
fn logical_gates_are_opt_in_absolute_and_bounded() {
    let yes = compile(
        "id='t'\n[[rules]]\nid='r'\nstate='working'\nall=[{region='whole_recent',logical_lines=true,contains=['abcdef']}]",
    );
    let no = compile("id='t'\n[[rules]]\nid='r'\nstate='working'\ncontains=['abcdef']");
    let screen = input("abc\ndef");
    assert!(yes.needs_logical_lines());
    assert!(!no.needs_logical_lines());
    assert_eq!(super::super::detect_with_logical_screen(&yes, &screen, Some("abcdef")).state, State::Working);
    assert_eq!(super::super::detect_with_logical_screen(&no, &screen, Some("abcdef")).state, State::Unknown);
    assert_eq!(detect(&yes, &screen).state, State::Unknown, "no view means no joining");
    let huge = "x".repeat(MAX_INPUT_BYTES + 1);
    assert_eq!(super::super::detect_with_logical_screen(&yes, &screen, Some(&huge)).state, State::Unknown);
    assert!(Manifest::compile("id='t'\n[[rules]]\nid='r'\nall=[{logical_lines=true,contains=['x']}]").is_err());
    for (id, _) in BUNDLED {
        assert_eq!(bundled(id).unwrap().needs_logical_lines(), *id == "command-code");
    }
}
