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
        "node" => script_arg_agent(&fg.argv, &NODE),
        "bun" => script_arg_agent(&fg.argv, &BUN),
        "deno" => script_arg_agent(&fg.argv, &DENO),
        name if is_python(name) => script_arg_agent(&fg.argv, &PYTHON),
        "sh" | "bash" | "zsh" | "fish" => script_arg_agent(&fg.argv, &SHELL),
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

/// One interpreter's flag semantics. Every table is per-runtime: node, bun,
/// deno, python and the shells give the same token different meanings, and an
/// option an interpreter does not know must end the scan, not become a guess.
struct Runtime {
    /// A subcommand that must precede the flags (`deno run`).
    subcommand: Option<&'static str>,
    /// Flags that evaluate inline source or answer a query — no entrypoint.
    eval: &'static [&'static str],
    /// Flags taking a module/package name (`-m`, attached or next arg).
    module: &'static [&'static str],
    /// Flags that consume the next argv item as their value.
    value: &'static [&'static str],
    /// Flags taking no value (long forms still accept `=value`).
    boolean: &'static [&'static str],
}

const NODE: Runtime = Runtime {
    subcommand: None,
    eval: &["-e", "--eval", "-p", "--print", "-c", "--check", "-v", "--version", "-h", "--help"],
    module: &[],
    value: &[
        "-r",
        "--require",
        "--loader",
        "--experimental-loader",
        "--import",
        "--inspect-port",
        "--icu-data-dir",
        "--openssl-config",
        "--redirect-warnings",
        "--policy",
        "-C",
        "--conditions",
        "--watch-path",
        "--env-file",
        "--env-file-if-exists",
        "--diagnostic-dir",
        "--entry-url",
        "--input-type",
        "--cpu-prof-name",
        "--cpu-prof-dir",
        "--heap-prof-name",
        "--heap-prof-dir",
        "--report-filename",
        "--report-directory",
        "--dns-result-order",
        "--test-concurrency",
        "--title",
    ],
    boolean: &[
        "--inspect",
        "--inspect-brk",
        "--inspect-wait",
        "-i",
        "--interactive",
        "--watch",
        "--test",
        "--experimental-strip-types",
        "--experimental-transform-types",
        "--experimental-detect-module",
        "--no-warnings",
        "--pending-deprecation",
        "--trace-deprecation",
        "--permission",
        "--frozen-intrinsics",
    ],
};

const BUN: Runtime = Runtime {
    subcommand: None,
    eval: &["-e", "--eval", "-p", "--print"],
    module: &[],
    value: &["-r", "--require", "--preload", "--import", "--define", "--env-file", "--cwd"],
    boolean: &["--watch", "--hot", "--smol", "-b", "--bun", "-i", "--inspect", "--inspect-brk", "--inspect-wait"],
};

const DENO: Runtime = Runtime {
    subcommand: Some("run"),
    eval: &[],
    module: &[],
    value: &[
        "-c",
        "--config",
        "--import-map",
        "--lock",
        "--cert",
        "-L",
        "--log-level",
        "--env",
        "--env-file",
        "--node-modules-dir",
        "--seed",
        "--location",
        "--v8-flags",
        "--inspect-port",
    ],
    boolean: &[
        "-A",
        "--allow-all",
        "--allow-*",
        "--deny-*",
        "--no-remote",
        "--cached-only",
        "--frozen",
        "--inspect",
        "--inspect-brk",
        "--inspect-wait",
        "--watch",
        "--check",
        "--no-check",
        "--no-npm",
        "--no-prompt",
        "--quiet",
        "-q",
        "--reload",
        "-r",
        "--unstable-*",
    ],
};

const PYTHON: Runtime = Runtime {
    subcommand: None,
    eval: &["-c", "-V", "--version", "-h", "--help", "--help-env", "--help-xoptions", "--help-all"],
    module: &["-m"],
    value: &["-W", "-X", "--check-hash-based-pycs"],
    boolean: &[
        "-B",
        "-b",
        "-d",
        "-E",
        "-i",
        "-I",
        "-O",
        "-OO",
        "-P",
        "-q",
        "-s",
        "-S",
        "-u",
        "-v",
        "--isolated",
        "--safe-path",
    ],
};

const SHELL: Runtime = Runtime {
    subcommand: None,
    eval: &["-c"],
    module: &[],
    value: &[],
    // Every other flag is unknown: `-l`, `-s` and friends change how the next
    // word is read, so a script path behind them is not safe to classify.
    boolean: &[],
};

/// Walk `argv` after the interpreter: the first positional token is the
/// script. Eval flags bail, module flags take the module, value flags skip
/// their value, known booleans pass — and anything unknown stops the scan.
fn script_arg_agent(argv: &[String], rt: &Runtime) -> Option<&'static str> {
    let mut args = argv.iter().skip(1).map(String::as_str);
    if let Some(sub) = rt.subcommand
        && args.next() != Some(sub)
    {
        return None;
    }
    while let Some(arg) = args.next() {
        if arg == "--" {
            return args.next().and_then(agent_from_entrypoint_path);
        }
        if !arg.starts_with('-') {
            return agent_from_entrypoint_path(arg);
        }
        if rt.eval.iter().any(|f| flag(arg, f, true).is_some()) {
            return None;
        }
        if let Some(module) = rt.module.iter().find_map(|f| flag(arg, f, true)) {
            return agent_from_module(match module {
                Some(attached) => attached,
                None => args.next()?,
            });
        }
        if let Some(inline) = rt.value.iter().find_map(|f| flag(arg, f, true)) {
            if inline.is_none() {
                let _ = args.next();
            }
            continue;
        }
        // Attached values only exist for long booleans (`--inspect=9229`);
        // a short flag with a tail is a different option entirely.
        if rt.boolean.iter().any(|f| flag(arg, f, f.starts_with("--")).is_some()) {
            continue;
        }
        return None;
    }
    None
}

/// Does `arg` spell `flag`? `Some(None)` is the bare flag whose value is the
/// next argv item, `Some(Some(v))` carries the value inline, `None` no match.
/// Entries ending in `*` are families (`--allow-*` covers `--allow-read`).
fn flag<'a>(arg: &'a str, flag: &str, attach: bool) -> Option<Option<&'a str>> {
    if let Some(prefix) = flag.strip_suffix('*') {
        let name = arg.split_once('=').map_or(arg, |(name, _)| name);
        return name.starts_with(prefix).then_some(None);
    }
    if arg == flag {
        return Some(None);
    }
    if !attach {
        return None;
    }
    if flag.starts_with("--") {
        arg.strip_prefix(flag).and_then(|rest| rest.strip_prefix('=')).map(Some)
    } else {
        arg.strip_prefix(flag).map(|rest| Some(rest.strip_prefix('=').unwrap_or(rest)))
    }
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
