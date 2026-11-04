//! Which supported agent the foreground process is, from `name` and `argv`.
//! Pure: the only syscall is canonicalizing a path token that might be an
//! install shim (`/usr/local/bin/cmd` → the real package entrypoint).

use crate::foreground::Foreground;

/// Every agent id the fork can detect (each keys a bundled manifest).
pub const SUPPORTED: &[&str] = &["claude", "codex", "command-code", "devin", "opencode"];

/// The foreground process's agent id, or `None` for shells and anything else.
/// Only argv[0] and an interpreter's script/module position count — an agent
/// named in a later argument is a mention, not evidence.
pub fn identify(fg: &Foreground) -> Option<&'static str> {
    let argv0 = fg.argv.first().map(String::as_str);

    // The comm name and argv0 basename, both cheap wins.
    for token in [fg.name.as_str()].into_iter().chain(argv0) {
        if let Some(agent) = agent_from_basename(token) {
            return Some(agent);
        }
    }

    // argv0 as a path: a package shim resolves to real entrypoint evidence.
    // Bare words never canonicalize — "cmd" must stay a shell.
    if let Some(path) = argv0
        && let Some(agent) = agent_from_entrypoint_path(path)
    {
        return Some(agent);
    }

    // The interpreter's script/module argument. `name` stands in for argv0
    // when argv is empty (procfs can lose it for a fast-exiting wrapper).
    let runtime = argv0.map(normalized_basename).or_else(|| Some(normalized_basename(&fg.name)))?;
    match runtime.as_str() {
        "node" | "bun" => script_arg_agent(&fg.argv, &["-e", "--eval", "-p", "--print"], &[]),
        name if is_python(name) => script_arg_agent(&fg.argv, &["-c"], &["-m"]),
        "sh" | "bash" | "zsh" | "fish" => script_arg_agent(&fg.argv, &["-c"], &[]),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// Basename, lowercased, one launcher suffix dropped (`.exe`, `.cmd`, ...).
fn normalized_basename(token: &str) -> String {
    let mut name = basename(token).to_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js"] {
        if let Some(stem) = name.strip_suffix(suffix) {
            name = stem.to_string();
            break;
        }
    }
    name
}

fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).find(|part| !part.is_empty()).unwrap_or(path)
}

/// A normalized basename that is itself an agent.
fn agent_from_basename(token: &str) -> Option<&'static str> {
    match normalized_basename(token).as_str() {
        "claude" | "claude-code" => Some("claude"),
        "codex" => Some("codex"),
        "command-code" => Some("command-code"),
        "devin" | "devin-cli" => Some("devin"),
        "opencode" | "opencode2" | "open-code" | "opencode-ai" => Some("opencode"),
        _ => None,
    }
}

/// An interpreter's script argument or a resolved shim path. Matches the
/// basename first, then a package directory under `node_modules` (how
/// `command-code` and `claude-code` installs lay out) — never a bare `cmd`,
/// a directory mention like `~/devin/`, or a scoped package on its own.
fn agent_from_entrypoint_path(path: &str) -> Option<&'static str> {
    if let Some(agent) = agent_from_basename(path) {
        return Some(agent);
    }
    if let Some(agent) = agent_from_package_components(path) {
        return Some(agent);
    }
    // Install shims (`…/bin/cmd` → the real .js) only resolve as real paths.
    if path.contains(['/', '\\']) {
        let resolved = std::fs::canonicalize(path).ok()?;
        let resolved = resolved.to_str()?;
        if resolved != path {
            return agent_from_basename(resolved).or_else(|| agent_from_package_components(resolved));
        }
    }
    None
}

/// `<…>/node_modules/<package>/<…>` → the package's agent. Scoped packages
/// match as `@scope/name` two-component windows; unscoped need one name.
fn agent_from_package_components(path: &str) -> Option<&'static str> {
    let parts: Vec<String> = path.split(['/', '\\']).filter(|p| !p.is_empty()).map(|p| p.to_lowercase()).collect();
    for window in parts.windows(3) {
        let [node_modules, a, b] = window else { continue };
        if node_modules != "node_modules" {
            continue;
        }
        match a.as_str() {
            "command-code" => return Some("command-code"),
            "claude-code" | "opencode-ai" => return agent_from_basename(a),
            "@openai" if b == "codex" => return Some("codex"),
            "@anthropic-ai" if b == "claude-code" => return Some("claude"),
            "@devin" | "@devin-ai" if b.starts_with("devin") => return Some("devin"),
            _ => {}
        }
    }
    None
}

fn is_python(name: &str) -> bool {
    name == "python"
        || name.strip_prefix("python").is_some_and(|v| {
            !v.is_empty() && v.split('.').all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        })
}

// ---------------------------------------------------------------------------
// Interpreter argv scanning
// ---------------------------------------------------------------------------

/// Walk `argv` after the interpreter: eval/module flags decide or bail, flags
/// with values skip two, the first positional token is the script or module.
fn script_arg_agent(argv: &[String], eval_flags: &[&str], module_flags: &[&str]) -> Option<&'static str> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--" {
            return args.next().and_then(|token| agent_from_entrypoint_path(token));
        }
        if flag_matches(arg, eval_flags) {
            return None; // `-e`/`-c` inline source: nothing inspectable
        }
        if flag_matches(arg, module_flags) {
            return args.next().and_then(|module| agent_from_module(module));
        }
        if arg.starts_with('-') {
            if option_takes_value(arg) {
                let _ = args.next();
            }
            continue;
        }
        return agent_from_entrypoint_path(arg);
    }
    None
}

/// `python -m pkg.mod` → the last dotted component, `_` folded to `-`.
fn agent_from_module(module: &str) -> Option<&'static str> {
    let last = module.rsplit('.').next()?.replace('_', "-");
    match last.as_str() {
        "devin" | "devin-cli" => Some("devin"),
        "claude" | "claude-code" => Some("claude"),
        "codex" => Some("codex"),
        "opencode" | "opencode2" | "open-code" | "opencode-ai" => Some("opencode"),
        "command-code" => Some("command-code"),
        _ => None,
    }
}

fn flag_matches(arg: &str, flags: &[&str]) -> bool {
    flags.iter().any(|flag| {
        arg == *flag
            // `-efoo` is `-e foo`
            || (!flag.starts_with("--") && arg.starts_with(*flag) && arg.len() > flag.len())
            // `--eval=foo` is `--eval foo`
            || (flag.starts_with("--") && arg.strip_prefix(*flag).is_some_and(|rest| rest.starts_with('=')))
    })
}

/// Flags the scan must skip a value for (node/python conventions).
fn option_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-r" | "--require"
            | "--loader"
            | "--import"
            | "--experimental-loader"
            | "--inspect-port"
            | "-W"
            | "-X"
            | "-S"
            | "-L"
            | "-o"
    )
}
