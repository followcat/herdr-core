use super::*;

fn composer(above: &str, pointer: &str, width: usize) -> String {
    let bar = "─".repeat(width);
    format!("{above}\n{bar}\n{pointer} Ask your question...\n{bar}\n  ? for shortcuts\n")
}

fn check(m: &Manifest, screen: &str, expected: State) {
    let d = detect(m, &input(screen));
    assert_eq!(d.state, expected, "{screen}\n{:?}", d.rule);
    if expected == State::Working {
        assert!(!m.rules.iter().find(|r| r.id == "composer_prompt").unwrap().matches(&input(screen)));
    } else if expected == State::Blocked {
        assert_eq!(d.kind, Some("permission"));
    }
}

#[test]
fn clipped_builtin_prefixes_share_the_working_and_idle_exclusion() {
    let m = bundled("command-code").unwrap();
    // Actual isolated Ink Status output, including a one-space counter gap.
    for (width, body) in [
        (45, " ○ Context limit reached. Auto-compacting the  1.1k"),
        (42, "○ Context limit reached. Auto-compacting the  1.1k"),
        (60, " ○ Context limit reached. Auto-compacting the conversation... 1.1k"),
        (45, " ○ Connection dropped mid-response. Retrying...  1.1k"),
        (18, "○ Context limit"),
    ] {
        check(&m, &composer(body, "❯", width), State::Working);
    }
    for label in
        ["Context limit reached. Auto-compacting the conversation...", "Checking for updates...", "Naming session…"]
    {
        let boundaries = label.char_indices().map(|(i, c)| i + c.len_utf8()).collect::<Vec<_>>();
        for &end in boundaries.iter().skip(5) {
            for suffix in ["", "  1.1k", "  esc to interrupt • 6s • ↓ 669"] {
                check(&m, &composer(&format!(" ○ {}{suffix}", &label[..end]), "❯", 45), State::Working);
            }
        }
    }
    for label in ["Connection Issue. Retrying (attempt 2)...", "Connection Issue.\nRetrying (attempt 2)..."] {
        for end in 6..=label.len() {
            check(&m, &composer(&label[..end], "❯", 24), State::Working);
        }
    }
    for body in [
        " ○ Conte",
        " ○ context limit reached.",
        "  ○ Context limit reached.",
        " ○ Context limit wrong",
        "Connection Ish",
        " ○ Custom mod status",
    ] {
        check(&m, &composer(body, "❯", 45), State::Idle);
    }
}

#[test]
fn prefix_matcher_accepts_only_prefixes_of_its_language() {
    let m = compile(
        r#"
        id = "prefix-test"
        min_engine_version = 7
        [[rules]]
        id = "prefix"
        state = "working"
        prefix = { regex = '^(?P<prefix>.+)$', min_chars = 3, patterns = ['ab(cd|ef){2,3}Z', '会話…', 'Error'] }
    "#,
    );
    for (text, state) in [
        ("abc", State::Working),
        ("abcdc", State::Working),
        ("abcdefefZ", State::Working),
        ("abcdcdcdZ", State::Working),
        ("会話…", State::Working),
        ("Error", State::Working),
        ("ab", State::Unknown),
        ("abd", State::Unknown),
        ("abcdZ", State::Unknown),
        ("abcdcdcdcd", State::Unknown),
        ("会話", State::Unknown),
        ("error", State::Unknown),
    ] {
        assert_eq!(detect(&m, &input(text)).state, state, "{text}");
    }
}

#[test]
fn prefix_matcher_keeps_manifest_limits_and_rejects_assertions() {
    for spec in [
        "regex='(?P<prefix>.+)', min_chars=0, patterns=['abc']",
        "regex='(?P<prefix>.+)', min_chars=513, patterns=['abc']",
        "regex='(?P<prefix>.+)', min_chars=3, patterns=[]",
        "regex='.+', min_chars=3, patterns=['abc']",
        "regex='(?P<prefix>.+)', min_chars=3, patterns=['a^']",
    ] {
        let text = format!("id='t'\n[[rules]]\nid='r'\nprefix = {{ {spec} }}");
        assert!(Manifest::compile(&text).is_err(), "{spec}");
    }
    for patterns in [vec!["a".repeat(513)], vec!["abc".to_owned(); 32], vec!["a[a-z]".repeat(60)]] {
        let text = format!(
            "id='t'\n[[rules]]\nid='r'\nprefix = {{ regex='(?P<prefix>.+)', min_chars=3, patterns={} }}",
            serde_json::to_string(&patterns).unwrap()
        );
        assert!(Manifest::compile(&text).is_err());
    }
}
