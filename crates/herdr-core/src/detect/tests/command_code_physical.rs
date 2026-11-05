use super::*;

fn physical_sweep(full_range: bool) {
    let data: serde_json::Value = serde_json::from_str(if full_range {
        include_str!("../../../tests/fixtures/detect/cmd-physical-sweep.json")
    } else {
        include_str!("../../../tests/fixtures/detect/cmd-width-sweep.json")
    })
    .unwrap();
    let manifest = bundled("command-code").unwrap();
    let mut terminal = None;
    let mut totals = std::collections::BTreeMap::<(u16, String), (usize, usize)>::new();
    let mut examples = Vec::new();
    for row in data["rows"].as_array().unwrap() {
        let (width, family, pointer, body) = if full_range {
            (
                row[0].as_u64().unwrap() as u16,
                row[1].as_str().unwrap(),
                row[2].as_str(),
                data["bodies"][row[3].as_u64().unwrap() as usize].as_str().unwrap(),
            )
        } else {
            (
                row["width"].as_u64().unwrap() as u16,
                row["family"].as_str().unwrap(),
                row["pointer"].as_str(),
                row["body"].as_str().unwrap(),
            )
        };
        if !matches!(&terminal, Some((w, _)) if *w == width) {
            terminal = Some((width, ghostty_vt::Terminal::new(width, 64, 0)));
        }
        let vt = &mut terminal.as_mut().unwrap().1;
        let working = matches!(family, "status" | "cloud" | "retry");
        let expected = if working {
            State::Working
        } else if family == "completed" {
            State::Idle
        } else {
            State::Blocked
        };
        for pointer in pointer.map_or_else(|| vec!["❯", ">"], |p| vec![p]) {
            let screen = if working {
                let bar = "─".repeat(width as usize);
                format!("{body}\n{bar}\n{pointer} Ask your question...\n{bar}\n  ? for shortcuts\n")
            } else {
                body.to_owned()
            };
            vt.write(b"\x1bc");
            vt.write(screen.replace('\n', "\r\n").as_bytes());
            let rows = vt.screen_rows();
            let logical = manifest.needs_logical_lines().then(|| vt.screen_logical_text());
            let verdict = crate::screen_status::detect_rows(&manifest, &rows, "", logical.as_deref());
            let pass = verdict.state == expected && (expected != State::Blocked || verdict.kind == Some("permission"));
            let count = totals.entry((width, family.to_owned())).or_default();
            count.0 += 1;
            if !pass {
                count.1 += 1;
                if examples.len() < 8 {
                    examples.push(format!(
                        "width={width} family={family} pointer={pointer} expected={expected:?} actual={:?}\n{}",
                        verdict.state,
                        rows.iter().map(|r| r.trim_end()).collect::<Vec<_>>().join("\n")
                    ));
                }
            }
        }
    }
    for ((width, family), (cases, failed)) in &totals {
        eprintln!("PHYSICAL {width} {family} {cases} {failed}");
    }
    let count: usize = totals.values().map(|v| v.0).sum();
    assert_eq!(count, if full_range { 40_670 } else { 4_586 });
    let failed: usize = totals.values().map(|v| v.1).sum();
    assert_eq!(failed, 0, "{}", examples.join("\n\n"));
}

#[test]
fn physical_rows_at_requested_widths() {
    physical_sweep(false);
}

#[test]
fn physical_rows_at_every_width() {
    physical_sweep(true);
}
