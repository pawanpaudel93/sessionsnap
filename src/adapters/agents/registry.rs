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

/// Which saved launch flags are passed again when resuming. Saved args are kept as cmux recorded
/// them (only flags) except exact tokens that pick a session: every agent's resume and continue
/// flags (see `AgentSpec::kept_args`), plus, per agent:
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flags {
    /// Claude Code: also `--session-id <id>`, `--fork-session`, `--print`/`-p`.
    Claude,
    /// Codex: also `resume|fork <uuid|--last>`, and all of a leading `exec` run.
    Codex,
    /// Nothing more.
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

    /// The saved launch flags that resuming passes again: all of them except the tokens that pick
    /// another session (the session would be given twice): per `Flags`, then the agent's own resume
    /// words, resume flags and continue flags. Codex's resume words are handled by `Flags::Codex`
    /// alone, so a flag value that happens to be "resume" is kept.
    pub fn kept_args(&self, args: &[String]) -> Vec<String> {
        match self.flags {
            Flags::Others => drop_resume_words(self, args),
            Flags::Claude => drop_resume_words(self, &claude_args(args)),
            Flags::Codex => codex_args(args),
        }
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

/// Claude Code: also `--session-id <id>`, `--fork-session` and `--print`/`-p`
/// (`--resume`/`-r` and `--continue`/`-c` are its resume and continue flags).
fn claude_args(args: &[String]) -> Vec<String> {
    let mut out = vec![];
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--fork-session" | "--print" | "-p" => {}
            "--session-id" => {
                it.next_if(|v| !v.starts_with('-'));
            }
            s if s.starts_with("--session-id=") => {}
            _ => out.push(a.clone()),
        }
    }
    out
}

/// Codex: `resume <x>` and `fork <x>` when `<x>` is a session ID or `--last`, and everything when
/// the first token is `exec` (a one-shot run). Anything else is kept, e.g. `-p resume x` (a profile
/// named "resume").
fn codex_args(args: &[String]) -> Vec<String> {
    if args.first().is_some_and(|a| a == "exec") {
        return vec![];
    }
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
        // Everything else stays as saved, values included.
        let args = strings(&["--effort", "high", "-w", "tree", "--debug", "api"]);
        assert_eq!(claude.kept_args(&args), args);
    }

    #[test]
    fn codex_drops_only_session_picking_tokens() {
        let codex = find("codex").unwrap();
        let flags = strings(&["-a", "on-request", "-s", "workspace-write", "--remote", "ws://h:1"]);
        assert_eq!(codex.kept_args(&flags), flags);
        let resumed = strings(&["resume", A, "-C", "/w", "-m", "o3"]);
        assert_eq!(codex.kept_args(&resumed), strings(&["-C", "/w", "-m", "o3"]));
        let late = strings(&["-m", "o3", "resume", A, "-s", "x"]);
        assert_eq!(codex.kept_args(&late), strings(&["-m", "o3", "-s", "x"]));
        let last = strings(&["-m", "gpt-5", "fork", "--last"]);
        assert_eq!(codex.kept_args(&last), strings(&["-m", "gpt-5"]));
        // A profile named "resume" is a flag value, not the subcommand.
        let profile = strings(&["-p", "resume", "x"]);
        assert_eq!(codex.kept_args(&profile), profile);
        // A one-shot `exec` run keeps nothing.
        assert_eq!(codex.kept_args(&strings(&["exec", "fix it", "--json"])), strings(&[]));
        assert_eq!(codex.kept_args(&strings(&["--json", "exec"])), strings(&["--json", "exec"]));
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
        assert_eq!(opencode.kept_args(&strings(&["-s", "old", "-c", "--model", "m"])), strings(&["--model", "m"]));
        assert_eq!(gemini.kept_args(&strings(&["-r", "old", "-m", "pro"])), strings(&["-m", "pro"]));
        // `-c` means something else for most agents: kept.
        assert_eq!(gemini.kept_args(&strings(&["-c"])), strings(&["-c"]));
        let grok = find("grok").unwrap();
        assert_eq!(grok.kept_args(&strings(&["--resume", "old", "-x"])), strings(&["-x"]));
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
