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
    /// Flags that evaluate inline source, run a non-file mode or answer a
    /// query — there is no script entrypoint (`-e`, `--run`, `--test`).
    eval: &'static [&'static str],
    /// Flags taking a module/package name (`-m`, attached or next arg).
    module: &'static [&'static str],
    /// Flags that consume the next argv item as their value.
    value: &'static [&'static str],
    /// Flags whose value is only accepted attached (`--flag=value`); the
    /// separate form is rejected by the interpreter, so a bare match fails
    /// closed and the next argv item is never its value.
    eq_value: &'static [&'static str],
    /// Flags taking no value (long forms still accept `=value`). A `--no-<x>`
    /// switch also passes when `--<x>` itself is listed here.
    boolean: &'static [&'static str],
}

// Flag arities checked against `node --help` and option probing on Node
// 24.11.1: `--entry-url` is a switch, `--no-<x>` is the negation of the
// boolean `--<x>` (there is no free `--no-*` family — node rejects unknown
// negations), V8-inherited size flags only accept `=`, `--run`/`--test` have
// no script entrypoint, and `--allow-fs-*` is the only permission family
// that takes a value. `--policy` is not a Node 24 option at all.
const NODE: Runtime = Runtime {
    subcommand: None,
    eval: &[
        "-e",
        "--eval",
        "-p",
        "--print",
        "-c",
        "--check",
        "--test",
        "--run",
        "-v",
        "--version",
        "-h",
        "--help",
        "--v8-options",
        "--completion-bash",
    ],
    module: &[],
    value: &[
        "-r",
        "--require",
        "--loader",
        "--experimental-loader",
        "--import",
        "-C",
        "--conditions",
        "--inspect-port",
        "--inspect-publish-uid",
        "--icu-data-dir",
        "--openssl-config",
        "--redirect-warnings",
        "--watch-path",
        "--watch-kill-signal",
        "--env-file",
        "--env-file-if-exists",
        "--diagnostic-dir",
        "--input-type",
        "--cpu-prof-name",
        "--cpu-prof-dir",
        "--cpu-prof-interval",
        "--heap-prof-name",
        "--heap-prof-dir",
        "--heap-prof-interval",
        "--heapsnapshot-near-heap-limit",
        "--heapsnapshot-signal",
        "--report-filename",
        "--report-directory",
        "--report-dir",
        "--report-signal",
        "--dns-result-order",
        "--test-concurrency",
        "--test-coverage-branches",
        "--test-coverage-exclude",
        "--test-coverage-functions",
        "--test-coverage-include",
        "--test-coverage-lines",
        "--test-global-setup",
        "--test-isolation",
        "--test-name-pattern",
        "--test-reporter",
        "--test-reporter-destination",
        "--test-rerun-failures",
        "--test-shard",
        "--test-skip-pattern",
        "--test-timeout",
        "--title",
        "--build-snapshot-config",
        "--disable-proto",
        "--disable-warning",
        "--experimental-config-file",
        "--experimental-sea-config",
        "--localstorage-file",
        "--max-http-header-size",
        "--max-old-space-size-percentage",
        "--network-family-autoselection-attempt-timeout",
        "--secure-heap",
        "--secure-heap-min",
        "--snapshot-blob",
        "--tls-cipher-list",
        "--tls-keylog",
        "--trace-event-categories",
        "--trace-event-file-pattern",
        "--trace-require-module",
        "--unhandled-rejections",
        "--use-largepages",
        "--v8-pool-size",
        "--allow-fs-*",
    ],
    // V8-inherited options reject the separate form outright.
    eq_value: &["--max-heap-size", "--max-old-space-size", "--max-semi-space-size", "--stack-size"],
    boolean: &[
        "--addons",
        "--allow-addons",
        "--allow-child-process",
        "--allow-wasi",
        "--allow-worker",
        "--build-snapshot",
        "--cpu-prof",
        "--heap-prof",
        "--inspect",
        "--inspect-brk",
        "--inspect-wait",
        "-i",
        "--interactive",
        "--interpreted-frames-native-stack",
        "--watch",
        "--watch-preserve-output",
        "--test-only",
        "--test-force-exit",
        "--test-update-snapshots",
        "--jitless",
        "--harmony",
        "--insecure-http-parser",
        "--napi-modules",
        "--permission",
        "--frozen-intrinsics",
        "--expose-gc",
        "--prof",
        "--prof-process",
        "--preserve-symlinks",
        "--preserve-symlinks-main",
        "--track-heap-objects",
        "--zero-fill-buffers",
        "--pending-deprecation",
        "--deprecation",
        "--warnings",
        "--throw-deprecation",
        "--report-compact",
        "--report-exclude-env",
        "--report-exclude-network",
        "--report-on-fatalerror",
        "--report-on-signal",
        "--report-uncaught-exception",
        "--trace-deprecation",
        "--trace-env",
        "--trace-env-js-stack",
        "--trace-env-native-stack",
        "--trace-exit",
        "--trace-promises",
        "--trace-sigint",
        "--trace-sync-io",
        "--trace-tls",
        "--trace-uncaught",
        "--trace-warnings",
        "--use-bundled-ca",
        "--use-env-proxy",
        "--use-openssl-ca",
        "--use-system-ca",
        "--enable-etw-stack-walking",
        "--enable-fips",
        "--enable-source-maps",
        "--force-fips",
        "--force-context-aware",
        "--force-node-api-uncaught-exceptions-policy",
        "--async-context-frame",
        "--extra-info-on-fatal-exception",
        "--force-async-hooks-checks",
        "--global-search-paths",
        "--network-family-autoselection",
        "--disable-sigusr1",
        "--disable-wasm-trap-handler",
        "--node-memory-debug",
        "--entry-url",
        "--experimental-eventsource",
        "--experimental-require-module",
        "--experimental-vm-modules",
        "--experimental-webstorage",
        "--experimental-strip-types",
        "--experimental-transform-types",
        "--experimental-detect-module",
        "--experimental-import-meta-resolve",
        "--experimental-network-inspection",
        "--experimental-print-required-tla",
        "--experimental-test-coverage",
        "--experimental-test-module-mocks",
        "--experimental-worker-inspection",
        "--experimental-default-config-file",
        "--experimental-global-navigator",
        "--experimental-repl-await",
        "--experimental-sqlite",
        "--experimental-websocket",
    ],
};

const BUN: Runtime = Runtime {
    subcommand: None,
    eval: &["-e", "--eval", "-p", "--print"],
    module: &[],
    value: &["-r", "--require", "--preload", "--import", "--define", "--env-file", "--cwd"],
    eq_value: &[],
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
    eq_value: &[],
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
    eq_value: &[],
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
    eq_value: &[],
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
        if let Some(inline) = rt.eq_value.iter().find_map(|f| flag(arg, f, true)) {
            // `=`-only flags: attached is fine, bare means the interpreter
            // would reject the option — fail closed, never consume.
            if inline.is_some() {
                continue;
            }
            return None;
        }
        // Attached values only exist for long booleans (`--inspect=9229`);
        // a short flag with a tail is a different option entirely.
        if rt.boolean.iter().any(|f| flag(arg, f, f.starts_with("--")).is_some()) {
            continue;
        }
        // `--no-<x>` stands only as the negation of a known boolean `--<x>`;
        // node rejects every other negation, so it fails closed here too.
        if let Some(rest) = arg.strip_prefix("--no-") {
            let positive = format!("--{}", rest.split('=').next().unwrap_or(rest));
            if rt.boolean.iter().any(|f| flag(&positive, f, true).is_some()) {
                continue;
            }
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
        let (name, value) = arg.split_once('=').map_or((arg, None), |(n, v)| (n, Some(v)));
        return name.starts_with(prefix).then_some(value);
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
