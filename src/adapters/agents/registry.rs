//! The coding agents sessionsnap can resume, and how. Resume commands are the ones cmux documents
//! for resuming each agent after a relaunch.

use serde::Serialize;

use crate::adapters::shell_word;
use crate::domain::flags::{self, Flag, FlagCheck, Policy};
use crate::domain::model::{has_control, is_session_id};

/// How much sessionsnap supports an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Support {
    /// Resumes its sessions and reads its session history (titles, `sessions`, `recent`).
    Full,
    /// Resumes its sessions with the command cmux documents; no history.
    ResumeOnly,
}

/// What a session ID of the agent looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ids {
    /// A UUID.
    Uuid,
    /// 1 to 200 characters from `[A-Za-z0-9._:-]`, starting with a letter or digit.
    Token,
}

/// Which tokens that pick a session are taken out of saved args before the agent's allow-list
/// (`AgentSpec::saved`) applies (see `AgentSpec::check_args`), per agent:
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flags {
    /// Claude Code: none; its session flags (`--resume`/`-r`, `--continue`/`-c`, `--session-id`,
    /// `--fork-session`) are silent entries of its flag table, read in the one parse, so the word
    /// after one is never taken as a value of the flag before it.
    Claude,
    /// Codex: also `resume|fork <uuid|--last>`.
    Codex,
    /// Its resume words, resume flags and continue flags (`drop_resume_words`).
    Others,
}

/// One agent: its names, how to resume a session and what sessionsnap knows about it.
#[derive(Debug)]
pub struct AgentSpec {
    /// The name sessionsnap saves, e.g. "codex".
    pub kind: &'static str,
    /// Other names it goes by: cmux hook aliases.
    pub aliases: &'static [&'static str],
    pub display: &'static str,
    /// The program that resumes it.
    pub program: &'static str,
    /// Arguments between the program and the session ID.
    pub resume: &'static [&'static str],
    /// Flags that pick a session by the ID after them (the template's flag and its other
    /// spellings); dropped with that ID from saved args.
    pub resume_flags: &'static [&'static str],
    /// Flags that continue the last session; dropped from saved args.
    pub continue_flags: &'static [&'static str],
    pub ids: Ids,
    pub flags: Flags,
    /// The flags that are saved and passed again on resume; everything else is left out.
    pub saved: Policy,
    pub support: Support,
}

const fn agent(
    kind: &'static str,
    aliases: &'static [&'static str],
    display: &'static str,
    program: &'static str,
    resume: &'static [&'static str],
    resume_flags: &'static [&'static str],
    continue_flags: &'static [&'static str],
) -> AgentSpec {
    AgentSpec {
        kind,
        aliases,
        display,
        program,
        resume,
        resume_flags,
        continue_flags,
        ids: Ids::Token,
        flags: Flags::Others,
        // Untested with sessionsnap: no flag is restored.
        saved: Policy::NONE,
        support: Support::ResumeOnly,
    }
}

/// Every agent sessionsnap knows, Full ones first.
pub const AGENTS: &[AgentSpec] = &[
    AgentSpec {
        kind: "claude",
        aliases: &[],
        display: "Claude Code",
        program: "claude",
        resume: &["--resume"],
        resume_flags: &["--resume", "-r"],
        continue_flags: &["--continue", "-c"],
        ids: Ids::Uuid,
        flags: Flags::Claude,
        saved: CLAUDE_FLAGS,
        support: Support::Full,
    },
    AgentSpec {
        kind: "codex",
        aliases: &[],
        display: "Codex",
        program: "codex",
        resume: &["resume"],
        resume_flags: &[],
        continue_flags: &[],
        ids: Ids::Uuid,
        flags: Flags::Codex,
        saved: CODEX_FLAGS,
        support: Support::Full,
    },
    agent("grok", &[], "Grok", "grok", &["-r"], &["-r", "--resume"], &[]),
    agent("opencode", &[], "OpenCode", "opencode", &["--session"], &["--session", "-s"], &["--continue", "-c"]),
    agent("pi", &[], "Pi", "pi", &["--session"], &["--session"], &[]),
    agent("omp", &[], "omp", "omp", &["--session"], &["--session"], &[]),
    agent("campfire", &[], "Campfire", "campfire", &["--session"], &["--session"], &[]),
    agent("amp", &[], "Amp", "amp", &["threads", "continue"], &[], &[]),
    agent("cursor", &[], "Cursor Agent", "cursor-agent", &["--resume"], &["--resume"], &[]),
    agent("gemini", &[], "Gemini CLI", "gemini", &["--resume"], &["--resume", "-r"], &[]),
    agent("antigravity", &["agy"], "Antigravity", "agy", &["--conversation"], &["--conversation"], &[]),
    agent("rovodev", &["rovo"], "Rovo Dev", "acli", &["rovodev", "run", "--restore"], &["--restore"], &[]),
    agent("hermes-agent", &[], "Hermes Agent", "hermes", &["--resume"], &["--resume", "-r"], &[]),
    agent("copilot", &[], "GitHub Copilot CLI", "copilot", &["--resume"], &["--resume", "-r"], &[]),
    agent("codebuddy", &[], "CodeBuddy", "codebuddy", &["--resume"], &["--resume", "-r"], &[]),
    agent("factory", &[], "Factory Droid", "droid", &["--resume"], &["--resume", "-r"], &[]),
    agent("qoder", &[], "Qoder CLI", "qodercli", &["--resume"], &["--resume", "-r"], &[]),
];

// The flags Claude Code and Codex resume with, checked against `claude --help` (2.1.296) and
// `codex --help` / `codex resume --help` (0.160.0). Arities and the launches that are not sessions
// follow cmux's `AgentLaunchSanitizer` policies. Each table lists every flag of the agent's command
// line (`dropped` and `skip` lines are known but not restored); a flag missing from it leaves out the
// session, since what follows it can't be read for certain. To restore another flag, add or change
// one line here. Mark
// `.restricting()` every flag that limits the agent (rules, sandboxes, modes, settings): when one
// can't be kept, the session is left out rather than resumed with fewer limits. `skip` lines are
// flags that are left out but take a value, so the value is never read as a prompt or a subcommand.
const CLAUDE_FLAGS: Policy = Policy {
    flags: &[
        Flag::word(&["--model"]),
        Flag::word(&["--fallback-model"]),
        Flag::word(&["--permission-mode"]).restricting(),
        Flag::word(&["--agent"]).restricting(),
        Flag::word(&["--effort"]),
        Flag::word(&["--setting-sources"]).restricting(),
        Flag::word(&["--max-budget-usd"]).restricting(),
        Flag::words(&["--betas"]),
        Flag::rules(&["--allowedTools", "--allowed-tools"]),
        Flag::rules(&["--tools"]).restricting(),
        Flag::rules(&["--disallowedTools", "--disallowed-tools"]).restricting(),
        Flag::paths(&["--add-dir"]),
        Flag::paths(&["--mcp-config"]),
        Flag::path(&["--settings"]).restricting(),
        Flag::path(&["--agents"]).restricting(),
        Flag::path(&["--plugin-dir"]).restricting(),
        Flag::path(&["--append-system-prompt-file"]),
        Flag::path(&["--system-prompt-file"]),
        Flag::switch(&["--strict-mcp-config"]).restricting(),
        Flag::switch(&["--restricted"]).restricting(),
        Flag::switch(&["--safe-mode"]).restricting(),
        Flag::switch(&["--disable-slash-commands"]).restricting(),
        Flag::switch(&["--bare"]).restricting(),
        Flag::switch(&["--dangerously-skip-permissions"]),
        Flag::switch(&["--allow-dangerously-skip-permissions"]),
        Flag::switch(&["--ide"]),
        Flag::switch(&["--verbose"]),
        Flag::switch(&["--chrome"]),
        Flag::switch(&["--no-chrome"]),
        Flag::skip(&["--append-system-prompt"]),
        Flag::skip(&["--system-prompt"]),
        Flag::skip(&["--autocompact"]),
        Flag::skip(&["--debug-file"]),
        Flag::skip(&["--environment"]),
        Flag::skip(&["--input-format"]),
        Flag::skip(&["--json-schema"]),
        Flag::skip(&["--name", "-n"]),
        Flag::skip(&["--output-format"]),
        Flag::skip(&["--permission-prompts"]),
        Flag::skip(&["--plugin-url"]),
        Flag::skip(&["--remote-control-session-name-prefix"]),
        Flag::skip(&["--session-id"]).silent(),
        Flag::skip(&["--system-prompt-snapshot"]),
        Flag::skip(&["--teammate-mode"]),
        Flag::skip_many(&["--file"]),
        Flag::skip_many(&["--dangerously-load-development-channels"]),
        // An optional value: the next word when it is not a flag.
        Flag::skip(&["--cloud"]).optional(),
        Flag::skip(&["--debug", "-d"]).optional(),
        Flag::skip(&["--from-pr"]).optional(),
        Flag::skip(&["--prompt-suggestions"]).optional(),
        Flag::skip(&["--remote-control"]).optional(),
        Flag::skip(&["--resume", "-r"]).optional().silent(),
        Flag::skip(&["--teleport"]).optional(),
        Flag::skip(&["--worktree", "-w"]).optional(),
        Flag::dropped(&["--ax-screen-reader"]),
        Flag::dropped(&["--bg", "--background"]),
        Flag::dropped(&["--brief"]),
        Flag::dropped(&["--continue", "-c"]).silent(),
        Flag::dropped(&["--desktop"]),
        Flag::dropped(&["--exclude-dynamic-system-prompt-sections"]),
        Flag::dropped(&["--fork-session"]).silent(),
        Flag::dropped(&["--forward-subagent-text"]),
        Flag::dropped(&["--include-hook-events"]),
        Flag::dropped(&["--include-partial-messages"]),
        Flag::dropped(&["--replay-user-messages"]),
        Flag::dropped(&["--tmux"]),
        Flag::dropped(&["--print", "-p"]),
        Flag::dropped(&["--no-session-persistence"]),
        Flag::dropped(&["--help", "-h"]),
        Flag::dropped(&["--version", "-v"]),
    ],
    commands: &[
        "agents",
        "api-key",
        "attach",
        "auth",
        "auto-mode",
        "config",
        "doctor",
        "gateway",
        "import",
        "install",
        "kill",
        "logs",
        "mcp",
        "plugin",
        "plugins",
        "purge",
        "rc",
        "remote-control",
        "respawn",
        "rm",
        "setup-token",
        "stop",
        "ultrareview",
        "update",
        "upgrade",
    ],
    rejects: &["--print", "-p", "--no-session-persistence", "--help", "-h", "--version", "-v"],
    dash_values: true,
    ..Policy::BASE
};

/// The `-c key=value` keys Codex restores.
const CODEX_CONFIG_KEYS: &[&str] =
    &["model", "model_provider", "model_reasoning_effort", "model_reasoning_summary", "model_verbosity"];

/// `-c` keys that neither limit nor loosen Codex: left out with a note. Any other key leaves out the
/// session, since it may limit Codex. An entry ending in `.` or `_` covers every key starting with it.
const CODEX_NEUTRAL_KEYS: &[&str] = &[
    "model_",
    "hide_agent_reasoning",
    "show_raw_agent_reasoning",
    "file_opener",
    "tui.",
    // Environment variables Codex adds to its shell: they limit nothing, and may hold secrets.
    "shell_environment_policy.set.",
    // A provider's definition may hold its credentials (`experimental_bearer_token`, `http_headers`,
    // `query_params`): never saved. A resume that picks the provider then fails, loudly.
    "model_providers.",
];

/// The `-c` keys that limit Codex, also restored; one that can't be kept loses the session. A key
/// ending in `.` stands for every key under it.
const CODEX_RESTRICTING_KEYS: &[&str] = &[
    "approval_policy",
    "sandbox_mode",
    "sandbox_workspace_write.",
    "profile",
    "features.",
    "shell_environment_policy.inherit",
    "shell_environment_policy.exclude",
    "shell_environment_policy.include_only",
    "shell_environment_policy.ignore_default_excludes",
];

const CODEX_FLAGS: Policy = Policy {
    flags: &[
        Flag::word(&["--model", "-m"]),
        Flag::word(&["--sandbox", "-s"]).restricting(),
        Flag::word(&["--ask-for-approval", "-a"]).restricting(),
        Flag::word(&["--profile", "-p"]).restricting(),
        Flag::word(&["--enable"]).restricting(),
        Flag::word(&["--disable"]).restricting(),
        Flag::config(&["--config", "-c"], CODEX_CONFIG_KEYS, CODEX_RESTRICTING_KEYS, CODEX_NEUTRAL_KEYS),
        Flag::path(&["--cd", "-C"]),
        Flag::path(&["--add-dir"]),
        Flag::switch(&["--dangerously-bypass-approvals-and-sandbox"]),
        Flag::switch(&["--search"]),
        Flag::switch(&["--oss"]),
        Flag::skip(&["--remote"]),
        Flag::skip(&["--remote-auth-token-env"]),
        Flag::skip(&["--local-provider"]),
        Flag::skip_many(&["--image", "-i"]),
        Flag::switch(&["--approve-for-me"]).restricting(),
        Flag::dropped(&["--strict-config"]),
        Flag::dropped(&["--dangerously-bypass-hook-trust"]),
        Flag::dropped(&["--worktree"]),
        Flag::dropped(&["--no-alt-screen"]),
        Flag::dropped(&["--no-daemon"]),
        Flag::dropped(&["--last"]),
        Flag::dropped(&["--all"]),
        Flag::dropped(&["--include-non-interactive"]),
        Flag::dropped(&["--help", "-h"]),
        Flag::dropped(&["--version", "-V"]),
    ],
    // `codex --help`'s subcommands other than `resume` and `fork` (taken out with their session).
    commands: &[
        "agents",
        "app",
        "app-server",
        "apply",
        "a",
        "archive",
        "cloud",
        "completion",
        "debug",
        "delete",
        "doctor",
        "e",
        "exec",
        "exec-server",
        "features",
        "help",
        "login",
        "logout",
        "mcp",
        "mcp-server",
        "migrate-rollouts",
        "plugin",
        "queue",
        "remote-control",
        "review",
        "sandbox",
        "unarchive",
        "update",
    ],
    rejects: &["--help", "-h", "--version", "-V"],
    ..Policy::BASE
};

/// The agent with this kind, alias or program name (any case).
pub fn find(name: &str) -> Option<&'static AgentSpec> {
    AGENTS.iter().find(|a| a.names().any(|n| n.eq_ignore_ascii_case(name)))
}

impl AgentSpec {
    /// Kind, aliases and program name.
    pub fn names(&self) -> impl Iterator<Item = &'static str> + '_ {
        std::iter::once(self.kind).chain(self.aliases.iter().copied()).chain(std::iter::once(self.program))
    }

    pub fn valid_id(&self, id: &str) -> bool {
        match self.ids {
            Ids::Uuid => is_session_id(id),
            // Starting with a letter or digit, so the ID is never read as a flag.
            Ids::Token => {
                id.len() <= 200
                    && id.starts_with(|c: char| c.is_ascii_alphanumeric())
                    && id.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
            }
        }
    }

    /// The saved launch flags that resuming passes again, and what was left out: per `Flags`, the
    /// tokens that pick another session go first (the session would be given twice; Claude Code's
    /// are silent entries of its table instead), then only the flags in `saved` are kept (see
    /// `domain::flags::check`). Codex's resume words are handled by `Flags::Codex` alone, so a flag
    /// value that happens to be "resume" is kept.
    pub fn check_args(&self, args: &[String]) -> FlagCheck {
        let picked = match self.flags {
            Flags::Others => drop_resume_words(self, args),
            // Claude Code's session flags are in its table (silent), read in the one parse.
            Flags::Claude => args.to_vec(),
            Flags::Codex => codex_args(args),
        };
        let mut check = flags::check(&picked, &self.saved);
        if self.flags == Flags::Codex {
            check.remarks.extend(dropped_providers(&picked));
        }
        check
    }

    /// The flags `check_args` keeps.
    pub fn kept_args(&self, args: &[String]) -> Vec<String> {
        self.check_args(args).kept
    }

    /// `[program, resume…, id, kept flags…]`, or None when `id` is not a valid ID for this agent, a
    /// flag has a control character (it would act on the terminal the command is typed into), or
    /// the saved flags lose the session (a deny rule that can't be kept, a launch that is not a
    /// session).
    pub fn resume_argv(&self, id: &str, args: &[String]) -> Option<Vec<String>> {
        if !self.valid_id(id) || args.iter().any(|a| has_control(a)) {
            return None;
        }
        let check = self.check_args(args);
        if check.lost.is_some() {
            return None;
        }
        let mut argv: Vec<String> =
            std::iter::once(self.program).chain(self.resume.iter().copied()).map(String::from).collect();
        argv.push(id.to_string());
        argv.extend(check.kept);
        Some(argv)
    }

    /// The resume command line, every word quoted for the shell.
    pub fn resume_command(&self, id: &str, args: &[String]) -> Option<String> {
        Some(self.resume_argv(id, args)?.iter().map(|w| shell_word(w)).collect::<Vec<_>>().join(" "))
    }

    /// The command's shape, e.g. "codex resume <id>".
    pub fn usage(&self) -> String {
        let mut words: Vec<&str> = std::iter::once(self.program).chain(self.resume.iter().copied()).collect();
        words.push("<id>");
        words.join(" ")
    }
}

impl AgentSpec {
    /// The session a launch names in its args, read the way `drop_resume_words` drops it: the
    /// template's words and the ID after them, a resume flag and its value, and Claude Code's
    /// `--session-id`. None when the launch names none, or names an invalid ID.
    pub fn session_in_args(&self, args: &[String]) -> Option<String> {
        // A fork runs under a new ID; the one in its args is the session it forked from.
        if args.iter().any(|a| a == "--fork-session") {
            return None;
        }
        let words: Vec<&str> = self.resume.iter().copied().take_while(|w| !w.starts_with('-')).collect();
        let has_flag = self.resume.last().is_some_and(|w| w.starts_with('-'));
        let id_flags: Vec<&str> =
            self.resume_flags.iter().copied().chain((self.kind == "claude").then_some("--session-id")).collect();
        let found = (0..args.len()).find_map(|i| {
            let a = args[i].as_str();
            if !has_flag && !words.is_empty() && args[i..].iter().take(words.len()).eq(words.iter()) {
                return args.get(i + words.len()).cloned();
            }
            if id_flags.contains(&a) {
                return args.get(i + 1).cloned();
            }
            id_flags.iter().find_map(|f| a.strip_prefix(f)?.strip_prefix('=').map(str::to_string))
        })?;
        self.valid_id(&found).then_some(found)
    }
}

/// Drops what would pick another session from saved args, for every agent:
/// - the template's leading words in a row (e.g. `threads continue`, `rovodev run`) — and, when the
///   template has no flag, the ID after them;
/// - each of `resume_flags` with the token after it (when that is not a flag), and their `=` forms;
/// - each of `continue_flags`.
///
/// Everything else is kept as saved: cmux records only the flags, so nothing is parsed further.
fn drop_resume_words(spec: &AgentSpec, args: &[String]) -> Vec<String> {
    let words: Vec<&str> = spec.resume.iter().copied().take_while(|w| !w.starts_with('-')).collect();
    let has_flag = spec.resume.last().is_some_and(|w| w.starts_with('-'));
    let mut out = vec![];
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if !words.is_empty() && args.len() - i >= words.len() && args[i..i + words.len()].iter().eq(words.iter()) {
            i += words.len();
            if !has_flag && args.get(i).is_some_and(|v| !v.starts_with('-')) {
                i += 1;
            }
            continue;
        }
        if spec.resume_flags.contains(&a) {
            i += 1;
            if args.get(i).is_some_and(|v| !v.starts_with('-')) {
                i += 1;
            }
            continue;
        }
        let inline = spec.resume_flags.iter().any(|f| a.strip_prefix(f).is_some_and(|r| r.starts_with('=')));
        if inline || spec.continue_flags.contains(&a) {
            i += 1;
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

/// "its model_providers.<id> settings were not saved", once per provider a Codex `-c` defined (those
/// keys are never saved). An id that is not a plain name is not shown.
fn dropped_providers(args: &[String]) -> Vec<String> {
    let mut ids: Vec<String> = vec![];
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        let value = match a.as_str() {
            "-c" | "--config" => it.next().map(String::as_str),
            _ => a.strip_prefix("--config=").or_else(|| a.strip_prefix("-c=")).or_else(|| a.strip_prefix("-c")),
        };
        let Some(rest) = value.and_then(|v| v.trim_start().strip_prefix("model_providers.")) else { continue };
        let id = rest.split(['.', '=']).next().unwrap_or("");
        let plain = !id.is_empty()
            && id.len() <= 32
            && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            && !flags::holds_credential(id);
        let shown = if plain { id.to_string() } else { "<provider>".to_string() };
        if !ids.contains(&shown) {
            ids.push(shown);
        }
    }
    ids.into_iter().map(|id| format!("its model_providers.{id} settings were not saved")).collect()
}

/// Codex: `resume <x>` and `fork <x>` when `<x>` is a session ID or `--last`, after the hook flags
/// cmux's wrapper puts first. Anything else is kept, e.g. `-p resume x` (a profile named "resume");
/// a one-shot `exec` run loses the session (see `CODEX_FLAGS`).
fn codex_args(args: &[String]) -> Vec<String> {
    let mut out = vec![];
    let mut i = 0;
    while i < args.len() {
        let pair = (args[i] == "resume" || args[i] == "fork")
            && args.get(i + 1).is_some_and(|x| is_session_id(x) || x == "--last");
        if pair {
            i += 2;
            continue;
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_session_a_launch_names() {
        let claude = find("claude").unwrap();
        let codex = find("codex").unwrap();
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(claude.session_in_args(&s(&["--model", "opus", "--resume", A])).as_deref(), Some(A));
        assert_eq!(claude.session_in_args(&s(&["-r", A])).as_deref(), Some(A));
        assert_eq!(claude.session_in_args(&s(&[&format!("--session-id={A}")])).as_deref(), Some(A));
        assert_eq!(codex.session_in_args(&s(&["--model", "o3", "resume", A])).as_deref(), Some(A));
        // None named, a picker without an ID, or an ID that is not one.
        assert_eq!(claude.session_in_args(&s(&["--continue"])), None);
        assert_eq!(claude.session_in_args(&s(&["--resume"])), None);
        assert_eq!(codex.session_in_args(&s(&["resume", "--last"])), None);
        assert_eq!(claude.session_in_args(&s(&["--resume", "x; rm -rf ~"])), None);
        assert_eq!(claude.session_in_args(&s(&["--resume", A, "--fork-session"])), None);
    }
    use crate::testing::{A, B};

    fn strings(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_agent_has_its_documented_resume_command() {
        let want = [
            ("claude", "claude --resume <id>"),
            ("codex", "codex resume <id>"),
            ("grok", "grok -r <id>"),
            ("opencode", "opencode --session <id>"),
            ("pi", "pi --session <id>"),
            ("omp", "omp --session <id>"),
            ("campfire", "campfire --session <id>"),
            ("amp", "amp threads continue <id>"),
            ("cursor", "cursor-agent --resume <id>"),
            ("gemini", "gemini --resume <id>"),
            ("antigravity", "agy --conversation <id>"),
            ("rovodev", "acli rovodev run --restore <id>"),
            ("hermes-agent", "hermes --resume <id>"),
            ("copilot", "copilot --resume <id>"),
            ("codebuddy", "codebuddy --resume <id>"),
            ("factory", "droid --resume <id>"),
            ("qoder", "qodercli --resume <id>"),
        ];
        assert_eq!(AGENTS.len(), want.len());
        for (spec, (kind, usage)) in AGENTS.iter().zip(want) {
            assert_eq!((spec.kind, spec.usage().as_str()), (kind, usage));
            let id = if spec.ids == Ids::Uuid { A } else { "T_1.2:3-x" };
            let cmd = spec.resume_command(id, &[]).unwrap();
            assert_eq!(cmd, usage.replace("<id>", id), "{kind}");
        }
        let full: Vec<&str> = AGENTS.iter().filter(|a| a.support == Support::Full).map(|a| a.kind).collect();
        assert_eq!(full, ["claude", "codex"]);
    }

    #[test]
    fn finds_agents_by_any_name() {
        assert_eq!(find("codex").unwrap().kind, "codex");
        assert_eq!(find("cursor-agent").unwrap().kind, "cursor");
        assert_eq!(find("agy").unwrap().kind, "antigravity");
        assert_eq!(find("rovo").unwrap().kind, "rovodev");
        assert_eq!(find("acli").unwrap().kind, "rovodev");
        assert_eq!(find("hermes").unwrap().kind, "hermes-agent");
        assert_eq!(find("droid").unwrap().kind, "factory");
        assert_eq!(find("qodercli").unwrap().kind, "qoder");
        assert_eq!(find("Claude").unwrap().kind, "claude");
        assert!(find("aider").is_none());
    }

    #[test]
    fn invalid_ids_get_no_command() {
        let claude = find("claude").unwrap();
        assert_eq!(claude.resume_command("x; rm -rf ~", &[]), None);
        assert_eq!(claude.resume_command("not-a-uuid", &[]), None);
        let amp = find("amp").unwrap();
        assert!(amp.resume_command("T-abc.1:2", &[]).is_some());
        for bad in ["", "a b", "a;b", "$(x)", "a'b", "a/b", &"x".repeat(201)] {
            assert_eq!(amp.resume_command(bad, &[]), None, "{bad:?}");
        }
        assert!(amp.valid_id(&"x".repeat(200)));
    }

    #[test]
    fn flags_with_control_characters_get_no_command() {
        let codex = find("codex").unwrap();
        for bad in ["\u{3}touch /tmp/pwned\r", "a\nb", "x\u{1b}[200~", "\u{7f}", "tab\there"] {
            assert_eq!(codex.resume_command(A, &strings(&["-m", bad])), None, "{bad:?}");
            // Even in a flag that would be left out.
            assert_eq!(codex.resume_command(A, &strings(&["--x", bad])), None, "{bad:?}");
        }
        // A backslash is no control character: quoted like any other word.
        assert_eq!(
            codex.resume_command(A, &strings(&["-C", r"/w/a\nb"])).unwrap(),
            format!(r"codex resume {A} -C '/w/a\nb'")
        );
    }

    #[test]
    fn saved_flags_are_quoted_for_the_shell() {
        let codex = find("codex").unwrap();
        let args = strings(&["-a", "on-request", "-C", "/my dir; rm -rf ~", "--add-dir", "$(id)"]);
        assert_eq!(
            codex.resume_command(A, &args).unwrap(),
            format!(r#"codex resume {A} -a on-request -C '/my dir; rm -rf ~' --add-dir '$(id)'"#)
        );
        let claude = find("claude").unwrap();
        assert_eq!(
            claude.resume_command(A, &strings(&["--allowedTools", "Bash(echo 'x'; id)"])).unwrap(),
            format!(r#"claude --resume {A} --allowedTools 'Bash(echo '"'"'x'"'"'; id)'"#)
        );
    }

    #[test]
    fn claude_drops_session_selecting_flags() {
        let claude = find("claude").unwrap();
        let args = strings(&[
            "-r",
            "old",
            "--model",
            "opus",
            "-c",
            "--session-id",
            "x",
            "--fork-session",
            "--resume=y",
            "--session-id=z",
            "--continue",
            "--add-dir",
            "/a",
            "--resume",
            "--verbose",
        ]);
        assert_eq!(claude.kept_args(&args), strings(&["--model", "opus", "--add-dir", "/a", "--verbose"]));
        // Session-picking flags are not reported as left out.
        assert_eq!(claude.check_args(&args).note("Claude Code"), None);
        // A one-shot run is not a session.
        assert_eq!(claude.resume_command(A, &strings(&["-p", "--model", "opus"])), None);
        assert_eq!(claude.resume_command(A, &strings(&["mcp", "serve"])), None);
    }

    /// Session-picking flags are read in the one parse, so the word after them is never taken as
    /// one more value of the flag before them: a prompt there is not saved.
    #[test]
    fn a_prompt_after_a_session_flag_is_not_saved() {
        let claude = find("claude").unwrap();
        for (args, kept) in [
            (
                strings(&["--add-dir", "/w", "--continue", "deploy; db password is hunter2"]),
                strings(&["--add-dir", "/w"]),
            ),
            (strings(&["--add-dir", "X", "-c", "keep going"]), strings(&["--add-dir", "X"])),
            (strings(&["--mcp-config", "a.json", "--resume", A, "finish it"]), strings(&["--mcp-config", "a.json"])),
            (strings(&["--mcp-config", "a.json", "-r", A, "finish it"]), strings(&["--mcp-config", "a.json"])),
            (
                strings(&["--allowedTools", "Edit", "--fork-session", "do the thing"]),
                strings(&["--allowedTools", "Edit"]),
            ),
            (
                strings(&["--disallowedTools", "Edit", "--session-id", A, "prompt words"]),
                strings(&["--disallowedTools", "Edit"]),
            ),
        ] {
            let c = claude.check_args(&args);
            assert_eq!((c.kept, c.lost), (kept, None), "{args:?}");
            assert_eq!(c.left_out, Vec::<String>::new(), "{args:?}");
        }
    }

    /// Commander gives a required option the next word whatever it starts with; clap does not.
    #[test]
    fn claude_required_values_may_start_with_a_dash() {
        let claude = find("claude").unwrap();
        let c = claude.check_args(&strings(&["--append-system-prompt", "--dangerously-skip-permissions"]));
        assert_eq!((c.kept, c.lost), (strings(&[]), None));
        let c = claude.check_args(&strings(&["--append-system-prompt", "- never push to main", "--model", "opus"]));
        assert_eq!((c.kept, c.lost), (strings(&["--model", "opus"]), None));
        let c = claude.check_args(&strings(&["--name", "--verbose", "--disallowedTools", "Bash"]));
        assert_eq!(c.kept, strings(&["--disallowedTools", "Bash"]));
        // An optional value still stops at a flag.
        let c = claude.check_args(&strings(&["--model", "opus", "-r", "--permission-mode", "plan"]));
        assert_eq!(c.kept, strings(&["--model", "opus", "--permission-mode", "plan"]));
        // Codex (clap): a dash word is a flag, so the sandbox has no value.
        let codex = find("codex").unwrap();
        assert!(codex.check_args(&strings(&["-s", "-m", "o3"])).lost.is_some());
    }

    /// The marks on single flags and `-c` families.
    #[test]
    fn restricting_marks() {
        let codex = find("codex").unwrap();
        // `set` only adds environment variables, and may hold secrets: never saved, only noted.
        let c = codex.check_args(&strings(&["-c", "shell_environment_policy.set.API_KEY=abc123secretvalue"]));
        assert_eq!((&c.kept, &c.lost, &c.left_out), (&strings(&[]), &None, &strings(&["-c"])));
        assert!(!c.note("Codex").unwrap().contains("abc123"));
        let kept =
            strings(&["-c", "shell_environment_policy.inherit=core", "-c", "shell_environment_policy.exclude=x"]);
        assert_eq!(codex.check_args(&kept).kept, kept);
        for bad in [
            strings(&["-c", "shell_environment_policy.experimental=x"]),
            strings(&["-c", "shell_environment_policy.inherit=a b"]),
            strings(&["--enable", "a b"]),
        ] {
            assert!(codex.check_args(&bad).lost.is_some(), "{bad:?}");
        }
        // A provider's settings may hold its credentials: never saved, and the note says so (a
        // resume that picks the provider then fails loudly).
        let provider = strings(&["-c", "model_provider=corp", "-c", "model_providers.corp.base_url=https://proxy"]);
        let c = codex.check_args(&provider);
        assert_eq!((&c.kept, &c.lost), (&strings(&["-c", "model_provider=corp"]), &None));
        assert!(c.note("Codex").unwrap().contains("its model_providers.corp settings were not saved"), "{c:?}");
        // Built at runtime so secret scanners don't flag the source.
        let token = format!("q{}Z", "7rT2".repeat(5));
        for field in [
            format!("experimental_bearer_token={token}"),
            format!("experimental_bearer_token=\"{token}\""),
            format!("http_headers.X-Api-Key={token}"),
            format!("query_params.api-key={token}"),
            format!("env_http_headers.X={token}"),
        ] {
            let args = vec!["-c".to_string(), format!("model_providers.corp.{field}")];
            let c = codex.check_args(&args);
            let shown = format!("{:?} {:?}", c.kept, c.note("Codex"));
            assert!(c.kept.is_empty() && c.lost.is_none() && !shown.contains(&token), "{args:?}: {shown}");
            assert!(!codex.resume_command(A, &args).unwrap().contains(&token), "{args:?}");
        }
        let claude = find("claude").unwrap();
        // Interactive `--agents` takes JSON, which can't be saved: the agent it defines may be the
        // one `--agent` picks, with its tool limits.
        let json = r#"{"r":{"description":"d","prompt":"p","tools":["Read"]}}"#;
        let c = claude.check_args(&strings(&["--agents", json, "--agent", "r"]));
        assert_eq!(c.lost, Some(flags::Lost::Restriction("--agents".into())));
        assert_eq!(claude.kept_args(&strings(&["--agents", "/w/a.json"])), strings(&["--agents", "/w/a.json"]));
        assert!(claude.check_args(&strings(&["--plugin-dir", "{x}"])).lost.is_some());
    }

    /// Nothing of cmux's is stripped: its exact hook settings can't be matched for certain (they
    /// differ by version and embed paths), and cmux 0.65 records the user's own argv. Any hook
    /// settings in `--settings` are inline JSON, so the restricting rule leaves out the session.
    #[test]
    fn hook_settings_in_claude_settings_lose_the_session() {
        let claude = find("claude").unwrap();
        for v in [
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"cmux claude-hook stop"}]}]}}"#,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"cmux claude-hook stop"}]}]},"permissions":{"deny":["Bash"]}}"#,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"cmux claude-hook stop --extra"}]}]}}"#,
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"/bin/deny # cmux claude-hook stop"}]}]}}"#,
            r#"{"preferredNotifChannel":"notifications_disabled"}"#,
            "x claude-hook y",
        ] {
            for args in [strings(&["--settings", v, "--model", "opus"]), vec![format!("--settings={v}")]] {
                let c = claude.check_args(&args);
                // A non-JSON value is a path: kept, as given.
                if !v.starts_with('{') {
                    assert_eq!(c.lost, None, "{args:?}");
                    continue;
                }
                assert_eq!(c.lost, Some(flags::Lost::Restriction("--settings".into())), "{args:?}");
            }
        }
    }

    #[test]
    fn codex_drops_only_session_picking_tokens() {
        let codex = find("codex").unwrap();
        let flags = strings(&["-a", "on-request", "-s", "workspace-write", "--search"]);
        assert_eq!(codex.kept_args(&flags), flags);
        let resumed = strings(&["resume", A, "-C", "/w", "-m", "o3"]);
        assert_eq!(codex.kept_args(&resumed), strings(&["-C", "/w", "-m", "o3"]));
        assert_eq!(codex.check_args(&resumed).note("Codex"), None);
        let late = strings(&["-m", "o3", "resume", A, "-s", "x"]);
        assert_eq!(codex.kept_args(&late), strings(&["-m", "o3", "-s", "x"]));
        let last = strings(&["-m", "gpt-5", "fork", "--last"]);
        assert_eq!(codex.kept_args(&last), strings(&["-m", "gpt-5"]));
        // A profile named "resume" is a flag value, not the subcommand.
        let profile = strings(&["-p", "resume"]);
        assert_eq!(codex.kept_args(&profile), profile);
        // A one-shot `exec` run (or another subcommand) is not a session: its sandbox flags would
        // be lost, so it is not resumed.
        for args in [strings(&["exec", "-s", "read-only", "fix it"]), strings(&["-m", "o3", "review"])] {
            assert!(matches!(codex.check_args(&args).lost, Some(flags::Lost::NotASession(_))), "{args:?}");
            assert_eq!(codex.resume_command(A, &args), None, "{args:?}");
        }
        // A prompt is not a subcommand.
        assert!(codex.resume_command(A, &strings(&["-s", "read-only", "fix the exec path"])).is_some());
        // cmux's injected hook flags are not stripped (0.65 records the user's own argv, and older
        // values can't be matched exactly): `hooks.*` is on no list, so the session is left out,
        // whether the value is cmux's, a user's, or cmux's with one value changed.
        let cmux = |event: &str, cmd: &str| {
            format!("hooks.{event}=[{{hooks=[{{type=\"command\",command='''{cmd}''',timeout=10000}}]}}]")
        };
        for args in [
            vec![
                "--enable".to_string(),
                "hooks".into(),
                "--dangerously-bypass-hook-trust".into(),
                "-c".into(),
                cmux("Stop", "/Users/me/.cmux/hooks/cmux-codex-hook-stop.sh"),
                "-m".into(),
                "o3".into(),
            ],
            vec![
                "--enable".into(),
                "hooks".into(),
                "--dangerously-bypass-hook-trust".into(),
                "-c".into(),
                cmux("Stop", "/Users/me/.cmux/hooks/cmux-codex-hook-stop.sh --x"),
            ],
            strings(&[
                "--enable",
                "hooks",
                "--dangerously-bypass-hook-trust",
                "-c",
                "hooks.PreToolUse=[{matcher=\"Bash\",hooks=[{type=\"command\",command=\"/usr/local/bin/deny-rm\"}]}]",
                "-a",
                "on-request",
            ]),
            strings(&["--enable=hooks", "--dangerously-bypass-hook-trust", "--config=hooks.PreToolUse=x"]),
        ] {
            assert!(matches!(codex.check_args(&args).lost, Some(flags::Lost::UnlistedConfigKey(..))), "{args:?}");
        }
        // Without the hook-trust flag that is the user's own `--enable`.
        assert_eq!(
            codex.kept_args(&strings(&["--enable", "hooks", "-m", "o3"])),
            strings(&["--enable", "hooks", "-m", "o3"])
        );
    }

    #[test]
    fn every_agent_drops_its_own_resume_words() {
        for spec in AGENTS {
            let id = if spec.ids == Ids::Uuid { A } else { "old-1" };
            // A flag each agent keeps (none for resume-only agents).
            let keep: &[&str] = match spec.kind {
                "claude" => &["--verbose"],
                "codex" => &["--search"],
                _ => &[],
            };
            let mut args = strings(keep);
            args.extend(spec.resume.iter().map(|w| w.to_string()));
            args.push(id.to_string());
            assert_eq!(spec.kept_args(&args), strings(keep), "{}", spec.kind);
            let new = if spec.ids == Ids::Uuid { B } else { "new-1" };
            let want = [spec.usage().replace("<id>", new)].into_iter().chain(keep.iter().map(|k| k.to_string()));
            assert_eq!(spec.resume_command(new, &args).unwrap(), want.collect::<Vec<_>>().join(" "), "{}", spec.kind);
        }
    }

    /// Resume-only agents are untested with sessionsnap: only a launch without flags resumes (an
    /// unknown flag such as gemini's `--sandbox` may limit the agent), and nothing else changes.
    #[test]
    fn resume_only_agents_resume_only_launches_without_flags() {
        for spec in AGENTS.iter().filter(|s| s.flags == Flags::Others) {
            for args in [strings(&["--sandbox"]), strings(&["--model", "m"]), strings(&["-x"])] {
                assert_eq!(spec.check_args(&args).lost, Some(flags::Lost::UntestedFlags), "{}", spec.kind);
                assert_eq!(spec.resume_command("s-1", &args), None, "{}", spec.kind);
            }
            // Their own resume words are not flags of the launch.
            let mut args: Vec<String> = spec.resume.iter().map(|w| w.to_string()).collect();
            args.push("old-1".into());
            assert_eq!(
                spec.resume_command("s-1", &args).unwrap(),
                spec.usage().replace("<id>", "s-1"),
                "{}",
                spec.kind
            );
        }
        assert_eq!(AGENTS.iter().filter(|s| s.flags == Flags::Others).count(), 15);
    }

    /// An old snapshot may hold flags saved before the allow-list: restore filters them the same way.
    #[test]
    fn restoring_old_snapshots_drops_flags_now_left_out() {
        let claude = find("claude").unwrap();
        let old = strings(&["--model", "opus", "--mcp-config", r#"{"env":{"X":"y"}}"#, "--name", "n", "--verbose"]);
        assert_eq!(claude.resume_command(A, &old).unwrap(), format!("claude --resume {A} --model opus --verbose"));
        // A flag the table doesn't know, or a cluster hiding a restriction: not resumed.
        assert_eq!(claude.resume_command(A, &strings(&["--api-key", "v"])), None);
        let codex = find("codex").unwrap();
        assert_eq!(codex.resume_command(A, &strings(&["-vs", "read-only"])), None);
        assert_eq!(codex.resume_command(A, &strings(&["--sandbox="])), None);
        // A restricting flag that can't be kept loses the session: no command.
        let ghp = format!("ghp_{}", "a1".repeat(18));
        assert_eq!(claude.resume_command(A, &strings(&["--disallowedTools", &format!("Bash({ghp})")])), None);
        assert_eq!(claude.resume_command(A, &strings(&["--settings", r#"{"permissions":{}}"#])), None);
        assert_eq!(claude.resume_command(A, &strings(&["--permission-mode", "plan mode"])), None);
        let old = strings(&["-c", "hide_agent_reasoning=true", "-m", "o3", "--remote", "ws://h:1"]);
        assert_eq!(codex.resume_command(A, &old).unwrap(), format!("codex resume {A} -m o3"));
        // A `-c` key on no list may limit Codex: not resumed.
        for key in ["mcp_servers.x.env={T=\"v\"}", "tools.web_search=false", "sandbox_permissions=[\"x\"]"] {
            assert_eq!(codex.resume_command(A, &strings(&["-c", key, "-m", "o3"])), None, "{key}");
        }
    }

    #[test]
    fn token_ids_never_look_like_flags() {
        let grok = find("grok").unwrap();
        assert!(!grok.valid_id("--yolo"));
        assert!(!grok.valid_id("-x"));
        assert!(!grok.valid_id(".x"));
        assert!(grok.valid_id("x-1"));
    }
}
