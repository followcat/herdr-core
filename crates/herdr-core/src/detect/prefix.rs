//! A captured label may be a complete match or a sufficiently long prefix
//! of the configured regular language. Layout stays in the manifest regex.

use regex::{Regex, RegexBuilder};
use regex_syntax::hir::{Hir, HirKind};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RawPrefix {
    pub regex: String,
    pub min_chars: usize,
    pub patterns: Vec<String>,
}

#[derive(Debug)]
pub(super) struct Prefix {
    shape: Regex,
    min_chars: usize,
    labels: Vec<(Regex, Regex)>,
}

impl Prefix {
    pub fn compile(raw: &RawPrefix, program_limit: usize) -> Result<Self, String> {
        if !(1..=512).contains(&raw.min_chars) || raw.patterns.is_empty() {
            return Err("prefix needs min_chars in 1..=512 and nonempty patterns".into());
        }
        let compile = |pattern: &str| {
            RegexBuilder::new(pattern).nest_limit(2048).size_limit(program_limit).build().map_err(|e| e.to_string())
        };
        let shape = compile(&raw.regex)?;
        if !shape.capture_names().any(|n| n == Some("prefix")) {
            return Err("prefix regex needs a named 'prefix' capture".into());
        }
        let mut labels = Vec::new();
        for pattern in &raw.patterns {
            let hir = regex_syntax::Parser::new().parse(pattern).map_err(|e| e.to_string())?;
            let partial = prefixes(&hir)?;
            labels.push((compile(&format!(r"\A(?:{pattern})\z"))?, compile(&format!(r"\A(?:{partial})\z"))?));
        }
        Ok(Self { shape, min_chars: raw.min_chars, labels })
    }

    pub fn matches(&self, text: &str) -> bool {
        self.shape.captures_iter(text).any(|captures| {
            let Some(captured) = captures.name("prefix") else { return false };
            let label = captured.as_str().split_whitespace().collect::<Vec<_>>().join(" ");
            let long_enough = label.chars().count() >= self.min_chars;
            self.labels.iter().any(|(full, partial)| full.is_match(&label) || long_enough && partial.is_match(&label))
        })
    }
}

// Prefix(A B) = Prefix(A) | A Prefix(B). A bounded repetition can stop
// inside its last occurrence; its minimum does not constrain a prefix.
// Reject positional assertions, whose truth depends on unseen input.
fn prefixes(hir: &Hir) -> Result<String, String> {
    const MAX_EXPANSION: usize = 16 * 1024;
    if hir.properties().minimum_len().is_none() {
        return Ok("[a&&b]".into());
    }
    let result = match hir.kind() {
        HirKind::Empty => String::new(),
        HirKind::Literal(literal) => {
            let text = std::str::from_utf8(&literal.0).map_err(|e| e.to_string())?;
            let mut tail = String::new();
            for ch in text.chars().rev() {
                tail = format!("(?:{}{tail})?", regex::escape(&ch.to_string()));
            }
            tail
        }
        HirKind::Class(_) => format!("(?:{hir})?"),
        HirKind::Capture(capture) => prefixes(&capture.sub)?,
        HirKind::Look(_) => return Err("prefix patterns cannot contain positional assertions".into()),
        HirKind::Alternation(parts) => {
            let mut options = Vec::new();
            let mut size = 0;
            for part in parts {
                let option = prefixes(part)?;
                size += option.len() + 1;
                if size > MAX_EXPANSION {
                    return Err("prefix expansion over 16 KiB".into());
                }
                options.push(option);
            }
            format!("(?:{})", options.join("|"))
        }
        HirKind::Concat(parts) => {
            let mut before = String::new();
            let mut options = Vec::new();
            let mut size = 0;
            for part in parts {
                let partial = prefixes(part)?;
                size += before.len() + partial.len() + 1;
                if size > MAX_EXPANSION {
                    return Err("prefix expansion over 16 KiB".into());
                }
                options.push(format!("{before}{partial}"));
                before.push_str(&format!("(?:{part})"));
            }
            format!("(?:{})", options.join("|"))
        }
        HirKind::Repetition(repeat) => match repeat.max {
            Some(0) => String::new(),
            Some(max) => format!("(?:{}){{0,{}}}(?:{})", repeat.sub, max - 1, prefixes(&repeat.sub)?),
            None => format!("(?:{})*(?:{})", repeat.sub, prefixes(&repeat.sub)?),
        },
    };
    if result.len() > MAX_EXPANSION {
        return Err("prefix expansion over 16 KiB".into());
    }
    Ok(result)
}
