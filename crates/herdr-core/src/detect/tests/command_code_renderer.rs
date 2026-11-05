use super::*;

fn expect(screen: &str, state: State, kind: Option<&str>) {
    let m = bundled("command-code").unwrap();
    let d = detect(&m, &input(screen));
    assert_eq!((d.state, d.kind), (state, kind), "screen: {screen}\nrule: {:?}", d.rule);
    if state == State::Working {
        assert!(!m.rules.iter().find(|r| r.id == "composer_prompt").unwrap().matches(&input(screen)));
    }
}

fn composer(above: &str, draft: &str, width: usize) -> String {
    let bar = "─".repeat(width);
    format!("{above}\n{bar}\n❯ {draft}\n{bar}\n  ? for shortcuts\n")
}

fn dialog(title: &str, question: &str, scoped: &str, risk: bool) -> String {
    let choices = if risk {
        "⚠ Destructive command. This is never auto-approved.\n❯ 1. Yes\n  2. No, tell Command Code what to do differently".into()
    } else {
        format!("❯ 1. Yes\n  2. {scoped}\n  3. No, tell Command Code what to do differently")
    };
    format!(
        "{}\n{title}\n{question}\n\n{choices}\n\n↑/↓ navigate · enter select · Run cmd --yolo to bypass all permissions (Docs ↗)\n",
        "─".repeat(80)
    )
}

fn shell(risk: bool) -> String {
    dialog(
        "Execute Shell Command",
        "Command Code needs to execute echo renderer.",
        "Yes, don't ask again for `echo` commands in this project",
        risk,
    )
}

const VERBS: &[&str] = &[
    "Pondering…",
    "Contemplating…",
    "Reasoning…",
    "Reflecting…",
    "Considering…",
    "Deliberating…",
    "Analyzing…",
    "Evaluating…",
    "Examining…",
    "Inspecting…",
    "Investigating…",
    "Reviewing…",
    "Researching…",
    "Studying…",
    "Exploring…",
    "Mapping…",
    "Tracing…",
    "Parsing…",
    "Processing…",
    "Calculating…",
    "Computing…",
    "Synthesizing…",
    "Planning…",
    "Outlining…",
    "Sketching…",
    "Drafting…",
    "Composing…",
    "Crafting…",
    "Building…",
    "Assembling…",
    "Constructing…",
    "Designing…",
    "Formulating…",
    "Structuring…",
    "Organizing…",
    "Preparing…",
    "Refining…",
    "Polishing…",
    "Honing…",
    "Tuning…",
    "Aligning…",
    "Connecting…",
    "Resolving…",
    "Weaving…",
    "Threading…",
    "Sculpting…",
    "Crystallizing…",
    "Channeling…",
    "Conjuring…",
    "Brewing…",
    "Working…",
    "Cogitating…",
    "Ruminating…",
    "Hypothesizing…",
    "Conceptualizing…",
    "Philosophizing…",
    "Deciphering…",
    "Demystifying…",
    "Articulating…",
    "Illuminating…",
    "Elaborating…",
    "Orchestrating…",
    "Choreographing…",
    "Architecting…",
    "Calibrating…",
    "Materializing…",
    "Visualizing…",
    "Harmonizing…",
    "Contemplificating…",
    "Supercalifragilisting…",
    "Bibbidibobbidibooing…",
    "Abracadabraing…",
    "Hocuspocusing…",
    "Razzmatazzing…",
];
const RESULTS: &[&str] = &[
    "Shell command allowed",
    "Shell command denied",
    "Process termination allowed",
    "Process termination denied",
    "Tool call allowed",
    "Tool call denied",
    "File operation allowed",
    "File operation denied",
];

#[test]
fn all_builtin_activity_labels_are_working_in_each_footer_mode() {
    let other = [
        "Executing: echo renderer",
        "Compacting conversation...",
        "Ready...",
        "Answers submitted",
        "Theme set to: dark",
        "Model set to renderer",
        "Provider set to renderer",
        "Provider authentication required. Use /connect to authenticate with OpenAI.",
        "Run /connect to authenticate with OpenAI first",
        "OpenAI authentication successful",
        "Failed to set provider",
        "Restore failed",
        "Connecting to the cloud…",
        "Loading the conversation…",
        "Loading the conversation",
        "Starting the cloud session",
        "Starting the cloud session…",
        "Cloning renderer…",
    ];
    for (i, status) in VERBS.iter().chain(RESULTS.iter()).chain(other.iter()).enumerate() {
        let glyph = ['·', '○', '◇', '☆', '✧', '⌘'][i % 6];
        for (width, suffix) in [(80, "  esc to interrupt • 6s • ↓ 669"), (60, "  1.1k"), (32, "")] {
            expect(
                &composer(&format!(" {glyph} {status}{suffix}"), "Ask your question...", width),
                State::Working,
                None,
            );
        }
    }
    expect(
        &composer(
            " ⌘ Cloning renderer…\n   Large repo: it is cloned once per session, and reopening is fast.",
            "Ask your question...",
            80,
        ),
        State::Working,
        None,
    );
    for dots in ["", ".", "..", "..."] {
        expect(
            &composer(&format!("Connection Issue. Retrying (attempt 2){dots}"), "Ask your question...", 80),
            State::Working,
            None,
        );
    }
    for unknown in [" ○ lowercase…", " ○ Custom mod status", " ○ Arbitrary extension text"] {
        expect(&composer(unknown, "Ask your question...", 80), State::Idle, None);
    }
}

#[test]
fn risk_filtered_choices_end_in_a_second_no_option() {
    let screen = shell(true);
    expect(&screen, State::Blocked, Some("permission"));
    expect(&screen.replace("❯ 1. Yes", "  1. Yes").replace("  2. No", "❯ 2. No"), State::Blocked, Some("permission"));
    // No must really be the final numbered choice, including when controls
    // wrap. A later Yes is not a completed permission-choice block.
    expect(&screen.replace("\n\n↑/↓", "\n  3. Yes\n\n↑/↓"), State::Unknown, None);
}
