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
    /// 1 to 200 characters from `[A-Za-z0-9._:-]`.
    Token,
}

/// Which saved launch flags are passed again when resuming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flags {
    /// Claude Code: all but -r/--resume <id>, -c/--continue, --session-id <id>, -p/--print and
    /// --fork-session.
    Claude,
    /// Codex: all but a leading subcommand and its positional arguments (`resume <id>`,
    /// `exec <prompt>`), and --last/--all after `resume`/`fork`.
    Codex,
    /// All of them, as saved.
    Verbatim,
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
        flags: Flags::Verbatim,
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
            Ids::Token => {
                !id.is_empty() && id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || "._:-".contains(c))
            }
        }
    }

    /// The saved launch flags that resuming passes again.
    pub fn kept_args(&self, args: &[String]) -> Vec<String> {
        match self.flags {
            Flags::Verbatim => args.to_vec(),
            Flags::Claude => claude_args(args),
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
            _ => out.push(a.clone()),
        }
    }
    out
}

fn codex_args(args: &[String]) -> Vec<String> {
    let Some(sub) = args.first().filter(|a| !a.starts_with('-')) else { return args.to_vec() };
    let rest = args[1..].iter().skip_while(|a| !a.starts_with('-'));
    let resuming = sub == "resume" || sub == "fork";
    rest.filter(|a| !(resuming && (*a == "--last" || *a == "--all"))).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::A;

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
    }

    #[test]
    fn resume_only_agents_keep_flags_verbatim() {
        let gemini = find("gemini").unwrap();
        let args = strings(&["--resume", "x", "-m", "pro"]);
        assert_eq!(gemini.kept_args(&args), args);
    }
}
