use super::*;

fn expect(screen: &str, state: State) {
    let manifest = bundled("command-code").unwrap();
    let verdict = detect(&manifest, &input(screen));
    assert_eq!(verdict.state, state, "screen: {screen}\nrule: {:?}", verdict.rule);
    if state == State::Working {
        assert!(!manifest.rules.iter().find(|r| r.id == "composer_prompt").unwrap().matches(&input(screen)));
    }
}

fn composer(above: &str, glyph: &str, draft: &str, width: usize) -> String {
    let bar = "─".repeat(width);
    format!("{above}\n{bar}\n{glyph} {draft}\n{bar}\n  ? for shortcuts\n")
}

#[test]
fn remaining_builtin_labels_suppress_idle_including_narrow_cloud_waits() {
    // Finite templates traced through all footer setters in cmd 1.88.3.
    let labels = [
        "Checking for updates...",
        "Updating 1.88.3 → 1.88.4",
        "Naming session…",
        "Setting effort to high in the cloud…",
        "Switching to Claude Opus 4.6 in the cloud…",
        "Failed to set model",
        "Failed to set theme",
        "Failed to set compact mode",
        "Language set to: English",
        "Could not save language: permission denied. Report issues at https://cond.dev/issue",
        "Restoring checkpoint...",
        "Removing share link...",
        "Connection dropped mid-response. Retrying... (2/3)",
        "Connection issue. Retrying... (attempt 2)",
        "Context limit reached. Auto-compacting the conversation...",
        "Connecting to the cloud",
        "Creating a cloud session…",
        "Restarting the cloud session…",
        "Compact mode set to: auto",
        "Waiting for your answer…",
        "Waiting for file edit permission...",
        "Waiting for shell command permission...",
        "Waiting for tool permission...",
        "Permission granted",
        "Permission denied",
        "Question cancelled",
        "Insufficient credits",
        "Model not available",
        "Premium credits exhausted",
        "Spend limit reached",
        "Usage limit reached",
        "Error",
        "Switched to Claude Sonnet",
        "No active session to unshare",
        "Failed to unshare: server unavailable",
        "Interrupted · What should Command Code do instead?",
    ];
    for label in labels {
        for (width, suffix) in [(80, "  esc to interrupt • 6s • ↓ 669"), (60, "  1.1k")] {
            expect(&composer(&format!(" ○ {label}{suffix}"), "❯", "", width), State::Working);
        }
    }
    // 32-column snapshots from isolated Ink renderToString trees with the
    // bundle's Status and ShimmerLine geometry; Command Code was not run.
    for rendered in [
        " ○ Checking for updates...",
        " ○ Updating 1.88.3 → 1.88.4",
        " ○ Naming session…",
        " ○Setting effort to high in the\n  cloud…",
        " ○Switching to Claude Opus 4.6\n  in the cloud…",
        " ○ Waiting for file edit",
        " ○ Waiting for shell command",
        " ○ Context limit reached.",
    ] {
        expect(&composer(rendered, "❯", "", 32), State::Working);
    }
    for fake in [
        " ○Setting effort to high in the\n\n  cloud…",
        " ○Switching to Claude Opus 4.6\n  unrelated continuation",
        " ○Switching to Claude Opus 4.6\n  cloud…",
        "  ○ Naming session…",
        " ○ Arbitrary extension text",
    ] {
        expect(&composer(fake, "❯", "", 32), State::Idle);
    }
}
#[test]
fn every_composer_glyph_isolates_trust_drafts_and_uncertain_boxes_fail_closed() {
    let trust = "Do you trust the files in this folder?\n❯ 1. Yes, proceed\n  2. No, exit\n↑/↓ to navigate · enter to select · esc to exit";
    for width in [80, 32] {
        // Normal, shell-input and figures' non-Unicode pointer fallback.
        // Plan/accept-edits affect hint text, not the input prefix.
        for glyph in ["!", ">", "❯"] {
            expect(&composer("Finished", glyph, trust, width), State::Idle);
            expect(&composer(" ○ Working…", glyph, "", width), State::Working);
        }
        let screen = composer("Finished", "λ", trust, width);
        expect(&screen, State::Unknown);
        expect(&screen.replace("  ? for shortcuts\n", ""), State::Unknown);
        for glyph in ["!", ">", "❯", "λ"] {
            let screen = composer("Finished", glyph, trust, width);
            let bar = "─".repeat(width);
            let unequal = screen.replacen(&bar, &"─".repeat(width + 1), 1);
            expect(&unequal, State::Unknown);
            // A viewport can clip either edge of a tall draft.
            let clipped_top = screen.replacen(&format!("Finished\n{bar}\n"), "", 1);
            expect(&clipped_top, State::Unknown);
            let clipped_bottom = screen.replace(&format!("\n{bar}\n  ? for shortcuts\n"), "");
            expect(&clipped_bottom, State::Unknown);
        }
    }
    expect(trust, State::Blocked);
    expect(&fixture("cmd-trust-dialog.txt"), State::Blocked);
}
#[test]
fn retry_message_accepts_only_adjacent_renderer_continuations() {
    for (width, body) in [
        (32, "Connection Issue. Retrying\n(attempt 2)"),
        (24, "Connection Issue.\nRetrying (attempt 2)"),
        (18, "Connection Issue.\nRetrying (attempt\n2)"),
    ] {
        for dots in ["", ".", "..", "..."] {
            for newline in ["\n", "\r\n"] {
                let rendered = format!("{body}{dots}").replace('\n', newline);
                expect(&composer(&rendered, "❯", "", width), State::Working);
                let padded = format!("{body}{dots}")
                    .lines()
                    .map(|line| format!("{line:width$}"))
                    .collect::<Vec<_>>()
                    .join(newline);
                expect(&composer(&padded, "❯", "", width), State::Working);
            }
        }
    }
    for fake in [
        "Connection Issue. Retrying\n\n(attempt 2)...",
        "Connection Issue. Retrying\nAssistant reply\n(attempt 2)...",
        "  Connection Issue. Retrying\n  (attempt 2)...",
        "Connection Issue. Retrying\n(attempt 2)...\nFinished reply",
    ] {
        expect(&composer(fake, "❯", "", 32), State::Idle);
    }
    expect(&composer("Finished reply", "❯", "Connection Issue. Retrying\n(attempt 2)...", 32), State::Idle);
}
