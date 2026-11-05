//! One agent's rules as strict TOML: `[[rules]]` with `state`, `priority`,
//! `region`, optional `kind` (blocked only), and matcher gates
//! (`contains`/`regex`/`line_regex`, nested `all`/`any`/`not`).
//! Same schema as herdr's manifests; compiled once into matchers and
//! pre-parsed regions, then shared through `Arc`.

use super::prefix::{Prefix, RawPrefix};
use super::{Input, State};
use regex::{Regex, RegexBuilder};
use serde::Deserialize;

/// Highest `min_engine_version` a manifest may require.
pub const ENGINE_VERSION: u32 = 7;

const MAX_RULES: usize = 128;
const MAX_GATE_DEPTH: usize = 8;
const MAX_TOTAL_GATES: usize = 512;
const MAX_MATCHERS_PER_GATE: usize = 32;
const MAX_TOTAL_MATCHERS: usize = 1024;
const MAX_MATCHER_CHARS: usize = 512;
/// Compiled-program bound per pattern; keeps pathological regexes cheap.
const MAX_REGEX_PROGRAM: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Permission,
    Question,
    Auth,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Permission => "permission",
            Kind::Question => "question",
            Kind::Auth => "auth",
        }
    }
}

/// A parsed, validated, compiled manifest.
#[derive(Debug)]
pub struct Manifest {
    pub id: String,
    pub aliases: Vec<String>,
    pub rules: Vec<Rule>,
}

/// One rule: a gate evaluated in a named region.
#[derive(Debug)]
pub struct Rule {
    pub id: String,
    pub state: State,
    pub kind: Option<Kind>,
    pub priority: i32,
    pub region: Region,
    pub visible_idle: bool,
    pub visible_blocker: bool,
    pub visible_working: bool,
    pub skip_state_update: bool,
    gate: Gate,
}

impl Rule {
    /// The gate's verdict on this rule's own region of the input.
    pub fn matches(&self, input: &Input<'_>) -> bool {
        let text = self.region.extract(input);
        self.gate.matches(text, input)
    }
}

/// A compiled matcher set. `contains` is stored lowercased and tested
/// against the lowercased region text; regexes run on the original.
#[derive(Debug, Default)]
struct Gate {
    region: Option<Region>,
    all: Vec<Gate>,
    any: Vec<Gate>,
    not: Vec<Gate>,
    contains: Vec<String>,
    regex: Vec<Regex>,
    line_regex: Vec<Regex>,
    prefix: Option<Prefix>,
}

impl Gate {
    fn matches(&self, text: &str, input: &Input<'_>) -> bool {
        let text = self.region.map_or(text, |r| r.extract(input));
        let lower = text.to_lowercase();
        self.matches_lower(text, &lower, input)
    }

    fn matches_lower(&self, text: &str, lower: &str, input: &Input<'_>) -> bool {
        let passes = |gate: &Gate| {
            if gate.region.is_some() { gate.matches(text, input) } else { gate.matches_lower(text, lower, input) }
        };
        if self.prefix.as_ref().is_some_and(|p| !p.matches(text)) {
            return false;
        }
        if !self.contains.iter().all(|n| lower.contains(n.as_str())) {
            return false;
        }
        if !self.regex.iter().all(|r| r.is_match(text)) {
            return false;
        }
        if !self.line_regex.iter().all(|r| text.lines().any(|line| r.is_match(line))) {
            return false;
        }
        if !self.all.iter().all(passes) {
            return false;
        }
        if !self.any.is_empty() && !self.any.iter().any(passes) {
            return false;
        }
        if self.not.iter().any(passes) {
            return false;
        }
        true
    }
}

// ---------------------------------------------------------------------------
// Raw TOML schema (unknown fields are rejected)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    id: String,
    #[allow(dead_code)]
    version: Option<String>,
    min_engine_version: Option<u32>,
    #[allow(dead_code)]
    updated_at: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    rules: Vec<RawRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    state: Option<String>,
    kind: Option<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default = "default_region")]
    region: String,
    #[serde(default)]
    visible_idle: bool,
    #[serde(default)]
    visible_blocker: bool,
    #[serde(default)]
    visible_working: bool,
    #[serde(default)]
    skip_state_update: bool,
    // The rule's own gate: same fields as RawGate, listed out so
    // deny_unknown_fields still rejects misspellings (flatten would not).
    #[serde(default)]
    all: Vec<RawGate>,
    #[serde(default)]
    any: Vec<RawGate>,
    #[serde(default, rename = "not")]
    not: Vec<RawGate>,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
    #[serde(default)]
    line_regex: Vec<String>,
    #[serde(default)]
    prefix: Option<RawPrefix>,
}

impl RawRule {
    fn gate(&self) -> RawGate {
        RawGate {
            region: None,
            all: self.all.clone(),
            any: self.any.clone(),
            not: self.not.clone(),
            contains: self.contains.clone(),
            regex: self.regex.clone(),
            line_regex: self.line_regex.clone(),
            prefix: self.prefix.clone(),
        }
    }
}

#[derive(Deserialize, Default, Clone)]
#[serde(deny_unknown_fields)]
struct RawGate {
    /// A nested gate may select an absolute region; otherwise it inherits
    /// the parent's region. Rule-level regions remain on Rule itself.
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    all: Vec<RawGate>,
    #[serde(default)]
    any: Vec<RawGate>,
    #[serde(default, rename = "not")]
    not: Vec<RawGate>,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
    #[serde(default)]
    line_regex: Vec<String>,
    #[serde(default)]
    prefix: Option<RawPrefix>,
}

fn default_region() -> String {
    "whole_recent".into()
}

fn state(value: Option<&str>) -> Result<State, String> {
    match value {
        None | Some("unknown") => Ok(State::Unknown),
        Some("idle") => Ok(State::Idle),
        Some("working") => Ok(State::Working),
        Some("blocked") => Ok(State::Blocked),
        Some(other) => Err(format!("state must be idle|working|blocked|unknown, not {other:?}")),
    }
}

fn kind(value: Option<&str>) -> Result<Option<Kind>, String> {
    match value {
        None => Ok(None),
        Some("permission") => Ok(Some(Kind::Permission)),
        Some("question") => Ok(Some(Kind::Question)),
        Some("auth") => Ok(Some(Kind::Auth)),
        Some(other) => Err(format!("kind must be permission|question|auth, not {other:?}")),
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Budget {
    gates: usize,
    matchers: usize,
}

impl RawGate {
    /// In positive context a gate needs a positive matcher; inside a `not`
    /// any non-empty gate is meaningful (a negated negation still states a
    /// condition).
    fn has_positive(&self) -> bool {
        !self.contains.is_empty()
            || !self.regex.is_empty()
            || !self.line_regex.is_empty()
            || self.prefix.is_some()
            || !self.all.is_empty()
            || !self.any.is_empty()
    }
    fn has_any(&self) -> bool {
        self.has_positive() || !self.not.is_empty()
    }
}

fn validate_gate(gate: &RawGate, depth: usize, budget: &mut Budget, ctx: &str, negative: bool) -> Result<(), String> {
    if depth > MAX_GATE_DEPTH {
        return Err(format!("{ctx}: gate depth over {MAX_GATE_DEPTH}"));
    }
    budget.gates += 1;
    if budget.gates > MAX_TOTAL_GATES {
        return Err(format!("over {MAX_TOTAL_GATES} gates total"));
    }
    let own = gate.contains.len()
        + gate.regex.len()
        + gate.line_regex.len()
        + gate.prefix.as_ref().map_or(0, |p| 1 + 2 * p.patterns.len());
    if own > MAX_MATCHERS_PER_GATE {
        return Err(format!("{ctx}: {own} matchers, over {MAX_MATCHERS_PER_GATE}"));
    }
    budget.matchers += own;
    if budget.matchers > MAX_TOTAL_MATCHERS {
        return Err(format!("over {MAX_TOTAL_MATCHERS} matchers total"));
    }
    for pat in gate
        .contains
        .iter()
        .chain(gate.regex.iter())
        .chain(gate.line_regex.iter())
        .chain(gate.prefix.iter().flat_map(|p| std::iter::once(&p.regex).chain(&p.patterns)))
    {
        if pat.chars().count() > MAX_MATCHER_CHARS {
            return Err(format!("{ctx}: matcher over {MAX_MATCHER_CHARS} chars"));
        }
    }
    if negative {
        if !gate.has_any() {
            return Err(format!("{ctx}: empty gate"));
        }
    } else if !gate.has_positive() {
        return Err(format!("{ctx}: needs a positive matcher"));
    }
    for sub in gate.all.iter().chain(gate.any.iter()) {
        validate_gate(sub, depth + 1, budget, ctx, negative)?;
    }
    for sub in &gate.not {
        validate_gate(sub, depth + 1, budget, ctx, true)?;
    }
    Ok(())
}

impl Manifest {
    /// Parse, validate and compile. Errors name the offending rule or field.
    pub fn compile(text: &str) -> Result<Manifest, String> {
        let raw: RawManifest = toml::from_str(text).map_err(|e| e.to_string())?;
        if raw.id.trim().is_empty() {
            return Err("id must not be empty".into());
        }
        if let Some(need) = raw.min_engine_version
            && need > ENGINE_VERSION
        {
            return Err(format!("needs engine {need}, this is {ENGINE_VERSION}"));
        }
        if raw.rules.is_empty() {
            return Err("no rules".into());
        }
        if raw.rules.len() > MAX_RULES {
            return Err(format!("{} rules, over {MAX_RULES}", raw.rules.len()));
        }
        let mut budget = Budget::default();
        let mut rules = Vec::with_capacity(raw.rules.len());
        for raw in &raw.rules {
            rules.push(compile_rule(raw, &mut budget)?);
        }
        Ok(Manifest { id: raw.id, aliases: raw.aliases, rules })
    }
}

fn compile_rule(raw: &RawRule, budget: &mut Budget) -> Result<Rule, String> {
    let at = |e: String| format!("rule {:?}: {e}", raw.id);
    if raw.id.trim().is_empty() {
        return Err("rule id must not be empty".into());
    }
    let state = state(raw.state.as_deref()).map_err(&at)?;
    let kind = kind(raw.kind.as_deref()).map_err(&at)?;
    if kind.is_some() && state != State::Blocked {
        return Err(at("kind is only valid with state = \"blocked\"".into()));
    }
    if raw.skip_state_update
        && (state != State::Unknown || raw.visible_idle || raw.visible_blocker || raw.visible_working)
    {
        return Err(at("skip_state_update only goes with state = \"unknown\" and no visible_*".into()));
    }
    let region = raw.region.trim().parse::<Region>().map_err(|e| at(format!("region {e}")))?;
    let gate_spec = raw.gate();
    validate_gate(&gate_spec, 0, budget, "gate", false).map_err(&at)?;
    let gate = compile_gate(&gate_spec).map_err(&at)?;
    Ok(Rule {
        id: raw.id.clone(),
        state,
        kind,
        priority: raw.priority,
        region,
        visible_idle: raw.visible_idle,
        visible_blocker: raw.visible_blocker,
        visible_working: raw.visible_working,
        skip_state_update: raw.skip_state_update,
        gate,
    })
}

fn compile_gate(raw: &RawGate) -> Result<Gate, String> {
    let rx = |pats: &[String]| -> Result<Vec<Regex>, String> {
        pats.iter()
            .map(|p| {
                RegexBuilder::new(p).size_limit(MAX_REGEX_PROGRAM).build().map_err(|e| format!("regex {p:?}: {e}"))
            })
            .collect()
    };
    Ok(Gate {
        region: raw.region.as_deref().map(str::parse).transpose()?,
        all: raw.all.iter().map(compile_gate).collect::<Result<_, _>>()?,
        any: raw.any.iter().map(compile_gate).collect::<Result<_, _>>()?,
        not: raw.not.iter().map(compile_gate).collect::<Result<_, _>>()?,
        contains: raw.contains.iter().map(|s| s.to_lowercase()).collect(),
        regex: rx(&raw.regex)?,
        line_regex: rx(&raw.line_regex)?,
        prefix: raw.prefix.as_ref().map(|p| Prefix::compile(p, MAX_REGEX_PROGRAM)).transpose()?,
    })
}

// ---------------------------------------------------------------------------
// Regions: named slices of the input (see regions.rs)
// ---------------------------------------------------------------------------

/// A parsed `region` value; extracted once per rule per evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    WholeRecent,
    AfterLastPromptMarker,
    BeforeCurrentPromptMarker,
    WholeRecentWithoutCurrentPromptMarker,
    CurrentPromptBlockMarker,
    AfterCurrentPromptBlockMarker,
    PromptBoxBody,
    FullWidthPromptBoxBody,
    AboveFullWidthPromptBox,
    OutsideFullWidthPromptBox,
    AfterLastFullWidthRule,
    AbovePromptBox,
    LastNonEmptyAbovePromptBox,
    CurrentShellBlock,
    AfterLastHorizontalRule,
    OscTitle,
    OscProgress,
    BottomLines(usize),
    BottomNonEmptyLines(usize),
    TopNonEmptyLines(usize),
}

impl Region {
    pub fn extract<'a>(&self, input: &Input<'a>) -> &'a str {
        use super::regions as r;
        match *self {
            Region::WholeRecent => input.screen,
            Region::AfterLastPromptMarker => r::after_last_prompt_marker(input.screen),
            Region::BeforeCurrentPromptMarker => r::before_current_prompt_marker(input.screen),
            Region::WholeRecentWithoutCurrentPromptMarker => r::without_current_prompt_marker(input.screen),
            Region::CurrentPromptBlockMarker => r::current_prompt_block_marker(input.screen),
            Region::AfterCurrentPromptBlockMarker => r::after_current_prompt_block_marker(input.screen),
            Region::FullWidthPromptBoxBody => r::full_width_prompt_box_body(input.screen),
            Region::OutsideFullWidthPromptBox => r::outside_full_width_prompt_box(input.screen),
            Region::AboveFullWidthPromptBox => r::above_full_width_prompt_box(input.screen),
            Region::AfterLastFullWidthRule => r::after_last_full_width_rule(input.screen),
            Region::PromptBoxBody => r::prompt_box_body(input.screen),
            Region::AbovePromptBox => r::above_prompt_box(input.screen),
            Region::LastNonEmptyAbovePromptBox => r::last_non_empty_above_prompt_box(input.screen),
            Region::CurrentShellBlock => r::current_shell_block(input.screen),
            Region::AfterLastHorizontalRule => r::after_last_horizontal_rule(input.screen),
            Region::OscTitle => input.title,
            Region::OscProgress => input.progress,
            Region::BottomLines(n) => r::bottom_lines(input.screen, n),
            Region::BottomNonEmptyLines(n) => r::bottom_non_empty_lines(input.screen, n),
            Region::TopNonEmptyLines(n) => r::top_non_empty_lines(input.screen, n),
        }
    }
}

impl std::str::FromStr for Region {
    type Err = String;
    fn from_str(spec: &str) -> Result<Region, String> {
        let spec = spec.trim();
        let count = |name: &str| spec.strip_prefix(name)?.strip_prefix('(')?.strip_suffix(')')?.parse::<usize>().ok();
        Ok(match spec {
            "whole_recent" => Region::WholeRecent,
            "after_last_prompt_marker" => Region::AfterLastPromptMarker,
            "before_current_prompt_marker" => Region::BeforeCurrentPromptMarker,
            "whole_recent_without_current_prompt_marker" => Region::WholeRecentWithoutCurrentPromptMarker,
            "current_prompt_block_marker" => Region::CurrentPromptBlockMarker,
            "after_current_prompt_block_marker" => Region::AfterCurrentPromptBlockMarker,
            "full_width_prompt_box_body" => Region::FullWidthPromptBoxBody,
            "outside_full_width_prompt_box" => Region::OutsideFullWidthPromptBox,
            "above_full_width_prompt_box" => Region::AboveFullWidthPromptBox,
            "after_last_full_width_rule" => Region::AfterLastFullWidthRule,
            "prompt_box_body" => Region::PromptBoxBody,
            "above_prompt_box" => Region::AbovePromptBox,
            "last_non_empty_above_prompt_box" => Region::LastNonEmptyAbovePromptBox,
            "current_shell_block" => Region::CurrentShellBlock,
            "after_last_horizontal_rule" => Region::AfterLastHorizontalRule,
            "osc_title" => Region::OscTitle,
            "osc_progress" => Region::OscProgress,
            _ => {
                if let Some(n) = count("bottom_lines") {
                    Region::BottomLines(n)
                } else if let Some(n) = count("bottom_non_empty_lines") {
                    Region::BottomNonEmptyLines(n)
                } else if let Some(n) = count("top_non_empty_lines") {
                    Region::TopNonEmptyLines(n)
                } else {
                    return Err(format!("unknown region {spec:?}"));
                }
            }
        })
    }
}
