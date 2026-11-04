//! Named slices of the visible screen. The screen input is the active pane's
//! plain text (one physical row per line, padding may trail). Functions
//! return `&str` subslices, `""` when the anchor is absent.

/// `bottom_lines(n)`: the last n lines including empty ones.
pub fn bottom_lines(content: &str, n: usize) -> &str {
    let count = content.lines().count();
    from_line(content, count.saturating_sub(n))
}

/// `bottom_non_empty_lines(n)`: from the n-th-from-last non-blank line on.
pub fn bottom_non_empty_lines(content: &str, n: usize) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(start) =
        lines.iter().enumerate().rev().filter(|(_, l)| !l.trim().is_empty()).take(n).last().map(|(i, _)| i)
    else {
        return "";
    };
    from_line(content, start)
}

/// `top_non_empty_lines(n)`: through the n-th non-blank line.
pub fn top_non_empty_lines(content: &str, n: usize) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(end) = lines.iter().enumerate().filter(|(_, l)| !l.trim().is_empty()).take(n).last().map(|(i, _)| i)
    else {
        return "";
    };
    &content[..line_offset(content, end + 1)]
}

/// `after_last_horizontal_rule`: text under the last ─-ruled line.
pub fn after_last_horizontal_rule(content: &str) -> &str {
    let mut cut = 0;
    for (i, line) in content.lines().enumerate() {
        if is_rule(line) {
            cut = line_offset(content, i + 1);
        }
    }
    &content[cut..]
}

/// `prompt_box_body`: between the second-to-last rule and the next rule
/// (the composer box's inside). Empty when no box is on screen.
pub fn prompt_box_body(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(top) = second_to_last_rule(&lines) else { return "" };
    let end = lines[top + 1..].iter().position(|l| is_rule(l)).map(|rel| top + 1 + rel).unwrap_or(lines.len());
    &content[line_offset(content, top + 1)..line_offset(content, end)]
}

/// `above_prompt_box`: everything above the composer's top rule; the whole
/// screen when no box exists.
pub fn above_prompt_box(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    match second_to_last_rule(&lines) {
        Some(top) => &content[..line_offset(content, top)],
        None => content,
    }
}

/// `last_non_empty_above_prompt_box`: the last non-blank line above the box.
pub fn last_non_empty_above_prompt_box(content: &str) -> &str {
    above_prompt_box(content).lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("")
}

/// `current_shell_block`: the last `!` shell-command block above the
/// composer — from its `! cmd` line to the box's edge. Older blocks' retained
/// controls stay outside the slice; "" when no `!` block is on screen.
pub fn current_shell_block(content: &str) -> &str {
    let above = above_prompt_box(content);
    let lines: Vec<&str> = above.lines().collect();
    let Some(at) = lines.iter().rposition(|l| l.starts_with("! ")) else { return "" };
    &above[line_offset(above, at)..]
}

// --- Codex prompt/block markers ------------------------------------------

/// `›` alone or starting a line: the Codex prompt.
fn prompt_line(line: &str) -> bool {
    line == "›" || line.starts_with("› ")
}

/// `•`/`■`/`✗`/`✓` at column zero: Codex's turn/result markers.
fn block_marker(line: &str) -> bool {
    line.starts_with('•') || line.starts_with('■') || line.starts_with('✗') || line.starts_with('✓')
}

/// The last prompt line with no block marker below it (still "current").
fn current_prompt_index(lines: &[&str]) -> Option<usize> {
    let at = lines.iter().rposition(|l| prompt_line(l))?;
    if lines[at + 1..].iter().any(|l| block_marker(l)) {
        return None;
    }
    Some(at)
}

/// `after_last_prompt_marker`: below the last `›` prompt (any prompt).
pub fn after_last_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    match lines.iter().rposition(|l| prompt_line(l)) {
        Some(at) => from_line(content, at + 1),
        None => content,
    }
}

/// `before_current_prompt_marker`: above the live prompt; whole screen when
/// none is live.
pub fn before_current_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    match current_prompt_index(&lines) {
        Some(at) => &content[..line_offset(content, at)],
        None => content,
    }
}

/// `whole_recent_without_current_prompt_marker`: the screen, unless a live
/// `›` prompt is up — then empty (the screen's tail is user-driven, not
/// agent work).
pub fn without_current_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    if current_prompt_index(&lines).is_some() { "" } else { content }
}

/// `current_prompt_block_marker`: the last block marker above the live
/// prompt; empty when none.
pub fn current_prompt_block_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(prompt) = current_prompt_index(&lines) else { return "" };
    lines[..prompt].iter().rev().find(|l| block_marker(l)).copied().unwrap_or("")
}

/// `after_current_prompt_block_marker`: from the last block marker above
/// the live prompt down; empty when either side is missing.
pub fn after_current_prompt_block_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(prompt) = current_prompt_index(&lines) else { return "" };
    let Some(at) = lines[..prompt].iter().rposition(|l| block_marker(l)) else { return "" };
    from_line(content, at)
}

// --- helpers --------------------------------------------------------------

/// A line that is all `─` (3+), optionally with a title on it.
fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    let n = trimmed.chars().take_while(|&c| c == '─').count();
    if n == 0 {
        return false;
    }
    let rest = trimmed.chars().skip(n).collect::<String>();
    rest.trim().is_empty() || n >= 3
}

fn second_to_last_rule(lines: &[&str]) -> Option<usize> {
    let mut seen = 0;
    for (i, line) in lines.iter().enumerate().rev() {
        if is_rule(line) {
            seen += 1;
            if seen == 2 {
                return Some(i);
            }
        }
    }
    None
}

fn from_line(content: &str, index: usize) -> &str {
    &content[line_offset(content, index)..]
}

/// The byte offset where line `index` begins, from real `\n` positions.
/// `lines()` strips `\r\n`, so `line.len() + 1` would drift one byte short
/// per line on CRLF input and slice mid-line — mid-char on UTF-8 text.
fn line_offset(content: &str, index: usize) -> usize {
    if index == 0 {
        return 0;
    }
    content.match_indices('\n').nth(index - 1).map(|(i, _)| i + 1).unwrap_or(content.len())
}
