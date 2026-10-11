//! The coding agents sessionsnap can resume, and how. Resume commands are the ones cmux documents
//! for resuming each agent after a relaunch.

use serde::Serialize;

use crate::adapters::shell_word;
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

/// Which saved launch flags are passed again when resuming. For every agent, the words of its own
/// resume command are dropped as well (see `AgentSpec::kept_args`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flags {
    /// Claude Code: also -r/--resume [id], -c/--continue, --session-id <id>, -p/--print,
    /// --fork-session, and positional arguments (an initial prompt).
    Claude,
    /// Codex: also the subcommand wherever it is among the positionals (`resume <id>`,
    /// `exec <prompt>`), its positional arguments, and --last/--all after `resume`/`fork`.
    Codex,
    /// The rest as saved.
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
    pub ids: Ids,
    pub flags: Flags,
    pub support: Support,
}

const fn agent(
    kind: &'static str,
    aliases: &'static [&'static str],
    display: &'static str,
    program: &'static str,
    resume: &'static [&'static str],
) -> AgentSpec {
    AgentSpec {
        kind,
        aliases,
        display,
        program,
        resume,
        ids: Ids::Token,
        flags: Flags::Others,
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
        ids: Ids::Uuid,
        flags: Flags::Claude,
        support: Support::Full,
    },
    AgentSpec {
        kind: "codex",
        aliases: &[],
        display: "Codex",
        program: "codex",
        resume: &["resume"],
        ids: Ids::Uuid,
        flags: Flags::Codex,
        support: Support::Full,
    },
    agent("grok", &[], "Grok", "grok", &["-r"]),
    agent("opencode", &[], "OpenCode", "opencode", &["--session"]),
    agent("pi", &[], "Pi", "pi", &["--session"]),
    agent("omp", &[], "omp", "omp", &["--session"]),
    agent("campfire", &[], "Campfire", "campfire", &["--session"]),
    agent("amp", &[], "Amp", "amp", &["threads", "continue"]),
    agent("cursor", &[], "Cursor Agent", "cursor-agent", &["--resume"]),
    agent("gemini", &[], "Gemini CLI", "gemini", &["--resume"]),
    agent("antigravity", &["agy"], "Antigravity", "agy", &["--conversation"]),
    agent("rovodev", &["rovo"], "Rovo Dev", "acli", &["rovodev", "run", "--restore"]),
    agent("hermes-agent", &[], "Hermes Agent", "hermes", &["--resume"]),
    agent("copilot", &[], "GitHub Copilot CLI", "copilot", &["--resume"]),
    agent("codebuddy", &[], "CodeBuddy", "codebuddy", &["--resume"]),
    agent("factory", &[], "Factory Droid", "droid", &["--resume"]),
    agent("qoder", &[], "Qoder CLI", "qodercli", &["--resume"]),
];

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

    /// The saved launch flags that resuming passes again: per `Flags`, then without the words of
    /// this agent's own resume command (the session would be given twice).
    pub fn kept_args(&self, args: &[String]) -> Vec<String> {
        let args = match self.flags {
            Flags::Others => args.to_vec(),
            Flags::Claude => claude_args(args),
            Flags::Codex => codex_args(args),
        };
        drop_resume_words(self.resume, &args)
    }

    /// `[program, resume…, id, kept flags…]`, or None when `id` is not a valid ID for this agent or a
    /// flag has a control character (it would act on the terminal the command is typed into).
    pub fn resume_argv(&self, id: &str, args: &[String]) -> Option<Vec<String>> {
        if !self.valid_id(id) || args.iter().any(|a| has_control(a)) {
            return None;
        }
        let mut argv: Vec<String> =
            std::iter::once(self.program).chain(self.resume.iter().copied()).map(String::from).collect();
        argv.push(id.to_string());
        argv.extend(self.kept_args(args));
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

/// Drops the words of a resume template from saved args. The template is subcommand words `S`
/// (e.g. `threads continue`, `rovodev run`) and an optional trailing flag `F` (e.g. `--session`).
/// - `S` is dropped wherever it appears in a row; without `F`, so is the ID after it.
/// - `F <value>` and `F=<value>` are dropped anywhere (the value only when it is not a flag).
fn drop_resume_words(template: &[&str], args: &[String]) -> Vec<String> {
    let words: Vec<&str> = template.iter().copied().take_while(|w| !w.starts_with('-')).collect();
    let flag = template.last().copied().filter(|w| w.starts_with('-'));
    let mut out = vec![];
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if !words.is_empty() && args.len() - i >= words.len() && args[i..i + words.len()].iter().eq(words.iter()) {
            i += words.len();
            if flag.is_none() && args.get(i).is_some_and(|v| !v.starts_with('-')) {
                i += 1;
            }
            continue;
        }
        if let Some(f) = flag {
            if a == f {
                i += 1;
                if args.get(i).is_some_and(|v| !v.starts_with('-')) {
                    i += 1;
                }
                continue;
            }
            if a.strip_prefix(f).is_some_and(|rest| rest.starts_with('=')) {
                i += 1;
                continue;
            }
        }
        out.push(args[i].clone());
        i += 1;
    }
    out
}

/// Claude Code flags that take one value.
const CLAUDE_VALUE_FLAGS: &[&str] = &[
    "--model",
    "--permission-mode",
    "--append-system-prompt",
    "--append-system-prompt-file",
    "--system-prompt",
    "--system-prompt-file",
    "--settings",
    "--output-format",
    "--input-format",
    "--fallback-model",
    "--agents",
    "--agent",
    "--max-turns",
    "--permission-prompt-tool",
    "--setting-sources",
    "--plugin-dir",
    "--max-budget-usd",
    "--json-schema",
    "--debug-file",
    "--name",
    "-n",
];
/// Claude Code flags that take one or more values.
const CLAUDE_LIST_FLAGS: &[&str] = &[
    "--add-dir",
    "--allowedTools",
    "--allowed-tools",
    "--disallowedTools",
    "--disallowed-tools",
    "--mcp-config",
    "--betas",
    "--tools",
];

fn claude_args(args: &[String]) -> Vec<String> {
    let mut out = vec![];
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-c" | "--continue" | "-p" | "--print" | "--fork-session" => {}
            // The ID after --resume is optional; without one Claude Code shows a picker.
            "-r" | "--resume" => {
                it.next_if(|v| !v.starts_with('-'));
            }
            "--session-id" => {
                it.next();
            }
            s if s.starts_with("--resume=") || s.starts_with("--session-id=") => {}
            s if CLAUDE_VALUE_FLAGS.contains(&s) => {
                out.push(a.clone());
                out.extend(it.next().cloned());
            }
            s if CLAUDE_LIST_FLAGS.contains(&s) => {
                out.push(a.clone());
                while let Some(v) = it.next_if(|v| !v.starts_with('-')) {
                    out.push(v.clone());
                }
            }
            s if s.starts_with('-') => out.push(a.clone()),
            // A positional is the initial prompt: never sent again.
            _ => {}
        }
    }
    out
}

/// Codex options that take a value (`--x=y` forms are one word).
const CODEX_VALUE_FLAGS: &[&str] = &[
    "-c",
    "--config",
    "-m",
    "--model",
    "-p",
    "--profile",
    "-a",
    "--ask-for-approval",
    "-s",
    "--sandbox",
    "-C",
    "--cd",
    "-i",
    "--image",
    "--add-dir",
    "--enable",
    "--disable",
    "--local-provider",
];

fn codex_args(args: &[String]) -> Vec<String> {
    let mut out = vec![];
    let mut sub: Option<&str> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if CODEX_VALUE_FLAGS.contains(&a.as_str()) {
            out.push(a.clone());
            out.extend(it.next().cloned());
        } else if a.starts_with('-') {
            let resuming = matches!(sub, Some("resume" | "fork"));
            if !(resuming && (a == "--last" || a == "--all")) {
                out.push(a.clone());
            }
        } else if sub.is_none() {
            // The first positional is the subcommand (or a prompt); both are dropped, and so are
            // the positionals after it.
            sub = Some(a.as_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
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
        }
        // A backslash is no control character: quoted like any other word.
        assert_eq!(
            codex.resume_command(A, &strings(&["-c", r"k=a\nb"])).unwrap(),
            format!(r"codex resume {A} -c 'k=a\nb'")
        );
    }

    #[test]
    fn saved_flags_are_quoted_for_the_shell() {
        let codex = find("codex").unwrap();
        let args = strings(&["-a", "on-request", "-c", "model=\"o3\"; rm -rf ~", "-C", "/my dir"]);
        assert_eq!(
            codex.resume_command(A, &args).unwrap(),
            format!(r#"codex resume {A} -a on-request -c 'model="o3"; rm -rf ~' -C '/my dir'"#)
        );
        let opencode = find("opencode").unwrap();
        assert_eq!(
            opencode.resume_command("s1", &strings(&["--model", "$(id)"])).unwrap(),
            "opencode --session s1 --model '$(id)'"
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
            "-p",
            "--fork-session",
            "--resume=y",
            "--session-id=z",
            "--continue",
            "--print",
            "--add-dir",
            "/a",
            "--resume",
            "--verbose",
        ]);
        assert_eq!(claude.kept_args(&args), strings(&["--model", "opus", "--add-dir", "/a", "--verbose"]));
        // An initial prompt (a positional) is never sent again; flag values are kept.
        let args = strings(&["--model", "opus", "fix the login bug", "--add-dir", "/a", "/b", "--verbose", "again"]);
        assert_eq!(claude.kept_args(&args), strings(&["--model", "opus", "--add-dir", "/a", "/b", "--verbose"]));
    }

    #[test]
    fn codex_drops_a_leading_subcommand_and_its_arguments() {
        let codex = find("codex").unwrap();
        let flags = strings(&["-a", "on-request", "-s", "workspace-write"]);
        assert_eq!(codex.kept_args(&flags), flags);
        let resumed = strings(&["resume", "old-id", "-C", "/w", "-m", "o3"]);
        assert_eq!(codex.kept_args(&resumed), strings(&["-C", "/w", "-m", "o3"]));
        let last = strings(&["resume", "--last", "-s", "read-only"]);
        assert_eq!(codex.kept_args(&last), strings(&["-s", "read-only"]));
        let exec = strings(&["exec", "fix it", "--json"]);
        assert_eq!(codex.kept_args(&exec), strings(&["--json"]));
        // A subcommand after global flags (their values are skipped).
        let late = strings(&["-m", "o3", "resume", "old", "-s", "x"]);
        assert_eq!(codex.kept_args(&late), strings(&["-m", "o3", "-s", "x"]));
        let late = strings(&["-c", "k=v", "-a", "never", "exec", "do it", "--json"]);
        assert_eq!(codex.kept_args(&late), strings(&["-c", "k=v", "-a", "never", "--json"]));
        let last = strings(&["-m", "gpt-5", "resume", "--last"]);
        assert_eq!(codex.kept_args(&last), strings(&["-m", "gpt-5"]));
        // A prompt given without a subcommand is not sent again.
        assert_eq!(codex.kept_args(&strings(&["-C", "/w", "fix the bug"])), strings(&["-C", "/w"]));
    }

    #[test]
    fn every_agent_drops_its_own_resume_words() {
        for spec in AGENTS {
            let id = if spec.ids == Ids::Uuid { A } else { "old-1" };
            let mut args = strings(&["--keep"]);
            args.extend(spec.resume.iter().map(|w| w.to_string()));
            args.push(id.to_string());
            args.push("--also".into());
            assert_eq!(spec.kept_args(&args), strings(&["--keep", "--also"]), "{}", spec.kind);
            let new = if spec.ids == Ids::Uuid { B } else { "new-1" };
            let want = format!("{} {new} --keep --also", spec.usage().trim_end_matches(" <id>"));
            assert_eq!(spec.resume_command(new, &args).unwrap(), want, "{}", spec.kind);
        }
        let rovodev = find("rovodev").unwrap();
        assert_eq!(rovodev.kept_args(&strings(&["rovodev", "run", "--yolo"])), strings(&["--yolo"]));
        assert_eq!(
            rovodev.resume_command("r1", &strings(&["rovodev", "run", "--yolo"])).unwrap(),
            "acli rovodev run --restore r1 --yolo"
        );
        let amp = find("amp").unwrap();
        assert_eq!(amp.kept_args(&strings(&["threads", "continue", "T-1"])), strings(&[]));
        let gemini = find("gemini").unwrap();
        assert_eq!(gemini.kept_args(&strings(&["--resume", "x", "-m", "pro"])), strings(&["-m", "pro"]));
        assert_eq!(gemini.kept_args(&strings(&["--resume=x", "--resume", "-m", "pro"])), strings(&["-m", "pro"]));
        let opencode = find("opencode").unwrap();
        assert_eq!(opencode.kept_args(&strings(&["--session", "s0", "--model", "m"])), strings(&["--model", "m"]));
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
