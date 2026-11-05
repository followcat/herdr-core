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
fn every_selected_choice_accepts_both_figures_pointers() {
    let m = bundled("command-code").unwrap();
    for pointer in ["❯", ">"] {
        for choice in ["1. Yes, proceed", "2. No, exit"] {
            let screen = format!(
                "Do you trust the files in this folder?\n{pointer} {choice}\n↑/↓ to navigate · enter to select · esc to exit"
            );
            check(&m, &screen, State::Blocked);
        }
    }
    for file in
        ["cmd-tool-permission.txt", "cmd-r3/edit-dialog-w80.txt", "cmd-r3/rm-dialog-w80.txt", "cmd-trust-dialog.txt"]
    {
        let screen = fixture(file);
        for pointer in ["❯", ">"] {
            let selected_yes = screen.replace('❯', pointer);
            check(&m, &selected_yes, State::Blocked);
            let selected_no = selected_yes
                .lines()
                .map(|line| {
                    if line.contains("1. Yes") {
                        line.replace(pointer, " ")
                    } else if line.trim_start().starts_with("2. No") || line.trim_start().starts_with("3. No") {
                        format!("{pointer} {}", line.trim_start())
                    } else {
                        line.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            check(&m, &selected_no, State::Blocked);
        }
    }
    let extra = fixture("cmd-r3/edit-dialog-w80.txt") + "\n> 4. Yes\n";
    check(&m, &extra, State::Unknown);
}

#[test]
fn generated_ink_width_sweep() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/detect/cmd-width-sweep.json")).unwrap();
    let m = bundled("command-code").unwrap();
    let idle = m.rules.iter().find(|r| r.id == "composer_prompt").unwrap();
    let mut totals = std::collections::BTreeMap::<(u64, String), (usize, usize)>::new();
    let mut failures = Vec::new();
    for row in data["rows"].as_array().unwrap() {
        let width = row["width"].as_u64().unwrap();
        let family = row["family"].as_str().unwrap();
        let body = row["body"].as_str().unwrap();
        let working = matches!(family, "status" | "cloud" | "retry");
        let expected = if working {
            State::Working
        } else if family == "completed" {
            State::Idle
        } else {
            State::Blocked
        };
        let pointers = row["pointer"].as_str().map_or_else(|| vec!["❯", ">"], |p| vec![p]);
        for pointer in pointers {
            let screen = if working { composer(body, pointer, width as usize) } else { body.to_owned() };
            let verdict = detect(&m, &input(&screen));
            let pass = verdict.state == expected
                && (expected != State::Blocked || verdict.kind == Some("permission"))
                && (!working || !idle.matches(&input(&screen)));
            let count = totals.entry((width, family.to_owned())).or_default();
            count.0 += 1;
            if !pass {
                count.1 += 1;
                if failures.len() < 35 {
                    failures.push(format!("w={width} pointer={pointer} {family} label={} mode={} selected={} expected={expected:?} actual={:?}\n{screen}", row["label"], row["mode"], row["selected"], verdict.state));
                }
            }
        }
    }
    for ((width, family), (cases, failed)) in &totals {
        eprintln!("SWEEP {width} {family} {cases} {failed}");
    }
    let failed: usize = totals.values().map(|v| v.1).sum();
    assert_eq!(failed, 0, "{}", failures.join("\n\n"));
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
