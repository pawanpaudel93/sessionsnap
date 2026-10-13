//! Which saved launch flags are kept: each agent lists the flags it allows (in the agent registry,
//! `adapters::agents::registry`), and every other flag is left out. A kept flag's values are still
//! checked: by their kind, and for the few credential formats that can be told apart for certain.
//!
//! Flag arities and the launches that can't be resumed follow cmux's `AgentLaunchSanitizer`
//! policies, checked against `claude --help` and `codex --help`.

/// What a listed flag takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Value {
    /// No value (`--verbose`).
    Switch,
    /// A short value such as a model name or a mode: letters, digits and `-_.:/@,`, no spaces, at
    /// most 100 characters.
    Word,
    /// A file or folder path; a value starting with `{` or `[` (inline JSON) is refused.
    Path,
    /// Permission rules (`Bash(git:*)`, `Edit`).
    Rules,
    /// `key=value` for one of these keys only, the value `Word`-shaped (optionally in double quotes).
    ConfigKey(&'static [&'static str]),
    /// Not restored, but known to take a value: the value goes with the flag (and is never read as
    /// a prompt or a subcommand).
    Skip,
}

/// One flag an agent knows: its spellings, what it takes and whether it is restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag {
    pub names: &'static [&'static str],
    pub value: Value,
    /// Takes one or more values (`--add-dir a b`).
    pub many: bool,
    /// A deny rule: when one of its values can't be kept, the whole session is left out.
    pub deny: bool,
}

impl Flag {
    pub const fn switch(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Switch, many: false, deny: false }
    }
    pub const fn word(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Word, many: false, deny: false }
    }
    pub const fn words(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Word, many: true, deny: false }
    }
    pub const fn path(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Path, many: false, deny: false }
    }
    pub const fn paths(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Path, many: true, deny: false }
    }
    pub const fn rules(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Rules, many: true, deny: false }
    }
    pub const fn deny_rules(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Rules, many: true, deny: true }
    }
    pub const fn config(names: &'static [&'static str], keys: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::ConfigKey(keys), many: false, deny: false }
    }
    pub const fn skip(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Skip, many: false, deny: false }
    }
    pub const fn skip_many(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Skip, many: true, deny: false }
    }
}

/// An agent's flag policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// The flags it knows; only those that are not `Value::Skip` are kept.
    pub flags: &'static [Flag],
    /// Subcommands that are not a session (`claude mcp …`): such a launch is not resumed.
    pub commands: &'static [&'static str],
    /// Flags that make a launch one that can't be resumed (`--print`).
    pub rejects: &'static [&'static str],
}

impl Policy {
    /// Keeps no flag: agents nobody has tested sessionsnap with.
    pub const NONE: Policy = Policy { flags: &[], commands: &[], rejects: &[] };
}

/// Why a whole agent session is left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lost {
    /// A value of a deny flag could not be kept: resuming without it would allow what was denied.
    DenyRule,
    /// The launch is a subcommand or a one-shot run (named here), not a session.
    NotASession(String),
}

/// The saved flags to keep, and what was left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FlagCheck {
    pub kept: Vec<String>,
    /// Names of the flags left out (each once, in order), never their values.
    pub left_out: Vec<String>,
    /// How many other words (not flags, nor values of known flags) were left out.
    pub other_words: usize,
    /// Set when the whole session must be left out.
    pub lost: Option<Lost>,
}

impl FlagCheck {
    /// Why the session is left out, in words, when it is.
    pub fn lost_reason(&self) -> Option<String> {
        self.lost.as_ref().map(|l| match l {
            Lost::DenyRule => "a deny rule could not be saved; resume it by hand".to_string(),
            Lost::NotASession(what) => format!("{what} is not a session that can be resumed"),
        })
    }

    /// What was left out, for `save`'s notes: flag names only, never a value.
    pub fn note(&self) -> Option<String> {
        if let Some(why) = self.lost_reason() {
            return Some(format!("left out its agent session ({why})"));
        }
        let others = match self.other_words {
            0 => String::new(),
            1 => "1 other argument".to_string(),
            n => format!("{n} other arguments"),
        };
        match (self.left_out.is_empty(), others.is_empty()) {
            (true, true) => None,
            (false, true) => Some(format!("left out flags it does not restore: {}", self.left_out.join(", "))),
            (false, false) => {
                Some(format!("left out flags it does not restore: {} (and {others})", self.left_out.join(", ")))
            }
            (true, false) => Some(format!("left out {others} it does not restore")),
        }
    }
}

/// `args` filtered through `policy`: listed flags whose values pass their checks are kept (in their
/// saved form, `--flag value` or `--flag=value`); everything else is left out. A dropped value of a
/// deny flag, a subcommand in `policy.commands` or a flag in `policy.rejects` loses the session.
pub fn check(args: &[String], policy: &Policy) -> FlagCheck {
    let mut out = FlagCheck::default();
    let find = |name: &str| policy.flags.iter().find(|f| f.names.contains(&name));
    let mut seen_word = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if a == "--" {
            out.other_words += args.len() - i;
            break;
        }
        if !a.starts_with('-') || a == "-" {
            if !seen_word && policy.commands.contains(&a) {
                out.lost = Some(Lost::NotASession(format!("the `{a}` command")));
            }
            seen_word = true;
            out.other_words += 1;
            continue;
        }
        let (name, inline) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (a, None),
        };
        if policy.rejects.contains(&name) {
            out.lost = Some(Lost::NotASession(format!("a launch with {name}")));
        }
        let Some(flag) = find(name) else {
            note_name(&mut out, name);
            continue;
        };
        if flag.value == Value::Switch {
            match inline {
                None => out.kept.push(a.to_string()),
                Some(_) => note_name(&mut out, name),
            }
            continue;
        }
        let values: Vec<&str> = match inline {
            Some(v) => vec![v],
            None => {
                let start = i;
                while i < args.len() && !args[i].starts_with('-') && (flag.many || i == start) {
                    i += 1;
                }
                args[start..i].iter().map(String::as_str).collect()
            }
        };
        if flag.value == Value::Skip {
            note_name(&mut out, name);
            continue;
        }
        let good: Vec<&str> = values.iter().copied().filter(|v| fits(flag.value, v) && !holds_credential(v)).collect();
        if good.len() < values.len() || values.is_empty() {
            note_name(&mut out, name);
            if flag.deny && good.len() < values.len() {
                out.lost = Some(Lost::DenyRule);
            }
        }
        if good.is_empty() {
            continue;
        }
        match inline {
            Some(_) => out.kept.push(a.to_string()),
            None => {
                out.kept.push(name.to_string());
                out.kept.extend(good.iter().map(|v| v.to_string()));
            }
        }
    }
    out
}

/// Records a left-out flag by its name, never by anything that could be a value: a short flag by
/// its letter (`-mo3` is `-m`), a long one only when it is a plain flag name.
fn note_name(out: &mut FlagCheck, name: &str) {
    let shown = if !name.starts_with("--") {
        let short: String = name.chars().take(2).collect();
        if short.chars().nth(1).is_some_and(|c| c.is_ascii_alphanumeric()) { short } else { "a flag".to_string() }
    } else if name.len() <= 64
        && name[2..].starts_with(|c: char| c.is_ascii_alphanumeric())
        && name[2..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !holds_credential(name)
    {
        name.to_string()
    } else {
        "a flag".to_string()
    };
    if !out.left_out.contains(&shown) {
        out.left_out.push(shown);
    }
}

/// `v` has the shape `kind` asks for.
fn fits(kind: Value, v: &str) -> bool {
    match kind {
        Value::Switch | Value::Skip => false,
        Value::Word => is_word(v),
        Value::Path => {
            let t = v.trim_start();
            !t.is_empty() && !t.starts_with('{') && !t.starts_with('[')
        }
        Value::Rules => true,
        Value::ConfigKey(keys) => v.split_once('=').is_some_and(|(k, value)| {
            let unquoted = value.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(value);
            keys.contains(&k) && is_word(unquoted)
        }),
    }
}

fn is_word(v: &str) -> bool {
    !v.is_empty() && v.len() <= 100 && v.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/@,".contains(c))
}

/// Prefixes specific to one token format (Anthropic, OpenAI projects, GitHub, GitLab), taken as a
/// token when 8 or more characters follow. Slack's `xox?-` is checked in `holds_credential`.
const TOKEN_PREFIXES: &[&str] =
    &["sk-ant-", "sk-proj-", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_", "glpat-"];

/// `v` holds, anywhere, a credential in a format that can be told apart for certain: a token with
/// a specific prefix (see `TOKEN_PREFIXES`, Slack `xox?-`, AWS `AKIA` plus 16, Google `AIza` plus
/// 35), a JWT, or a URL with a password (`scheme://user:pass@host`).
pub fn holds_credential(v: &str) -> bool {
    token_in(v) || url_with_password(v)
}

fn token_in(v: &str) -> bool {
    v.split(|c: char| !(c.is_ascii_alphanumeric() || "_-.".contains(c))).any(|word| {
        // A word may run on into a dot (a sentence's end); prefixes hold none.
        let plain = word.split('.').next().unwrap_or("");
        let lower = plain.to_ascii_lowercase();
        let b = plain.as_bytes();
        // A prefix counts at the word's start or after a `-` or `_` in it (`opus-ghp_…`).
        let prefixed = TOKEN_PREFIXES.iter().any(|p| {
            lower.match_indices(p).any(|(i, _)| {
                (i == 0 || !lower.as_bytes()[i - 1].is_ascii_alphanumeric()) && lower.len() >= i + p.len() + 8
            })
        });
        let slack = b.len() >= 13 && lower.starts_with("xox") && b[3].is_ascii_alphabetic() && b[4] == b'-';
        let aws = plain.starts_with("AKIA")
            && b.len() >= 20
            && b[4..20].iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        let google = plain.starts_with("AIza")
            && b.len() >= 39
            && b[4..39].iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-' || *c == b'_');
        let parts: Vec<&str> = word.split('.').collect();
        let jwt = word.starts_with("eyJ") && parts.len() == 3 && parts.iter().all(|p| !p.is_empty());
        prefixed || slack || aws || google || jwt
    })
}

/// `v` holds a URL with a password: `scheme://user:pass@host` (or `scheme://:pass@host`).
fn url_with_password(v: &str) -> bool {
    v.match_indices("://").any(|(i, _)| {
        let rest = &v[i + 3..];
        let authority = rest.split(|c: char| "/?#\"' \\".contains(c) || c.is_whitespace()).next().unwrap_or("");
        authority
            .rsplit_once('@')
            .and_then(|(userinfo, _)| userinfo.split_once(':'))
            .is_some_and(|(_, password)| !password.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    const POLICY: Policy = Policy {
        flags: &[
            Flag::word(&["--model", "-m"]),
            Flag::switch(&["--verbose"]),
            Flag::path(&["--settings"]),
            Flag::paths(&["--add-dir"]),
            Flag::rules(&["--allowedTools"]),
            Flag::deny_rules(&["--disallowedTools"]),
            Flag::config(&["-c"], &["model"]),
            Flag::skip(&["--name"]),
        ],
        commands: &["mcp"],
        rejects: &["--print"],
    };

    #[test]
    fn keeps_listed_flags_in_their_saved_form() {
        let args = words(&[
            "--model",
            "o3",
            "--verbose",
            "--add-dir",
            "/a",
            "/b c",
            "--settings=~/s.json",
            "-c",
            "model=\"o3\"",
        ]);
        assert_eq!(check(&args, &POLICY), FlagCheck { kept: args.clone(), ..Default::default() });
    }

    #[test]
    fn values_must_fit_their_kind() {
        for args in [
            words(&["--model", "two words"]),
            words(&["--model", &"x".repeat(101)]),
            words(&["--settings", r#"{"a":1}"#]),
            words(&["--settings", " [1]"]),
            words(&["-c", "other=1"]),
            words(&["-c", "model=a b"]),
            words(&["--verbose=1"]),
            words(&["--model"]),
        ] {
            let c = check(&args, &POLICY);
            assert!(c.kept.is_empty(), "{args:?}: {c:?}");
            assert_eq!(c.left_out.len(), 1, "{args:?}");
        }
        // A bad value goes; the flag stays with the others.
        let c = check(&words(&["--add-dir", "/a", "[x]", "/b"]), &POLICY);
        assert_eq!(c.kept, words(&["--add-dir", "/a", "/b"]));
        assert_eq!(c.left_out, ["--add-dir"]);
    }

    /// A single-value flag takes one word: a following prompt word is not its value.
    #[test]
    fn single_value_flags_take_one_word() {
        let c = check(&words(&["--model", "o3", "fix it", "--name", "n", "x"]), &POLICY);
        assert_eq!(c.kept, words(&["--model", "o3"]));
        assert_eq!((c.left_out, c.other_words), (words(&["--name"]), 2));
    }

    #[test]
    fn notes_name_flags_never_values() {
        let c = check(&words(&["--token=abc", "-kSECRET", "--x", "y"]), &POLICY);
        assert_eq!(c.left_out, ["--token", "-k", "--x"]);
        assert_eq!(c.note().unwrap(), "left out flags it does not restore: --token, -k, --x (and 1 other argument)");
        let ghp = format!("--ghp_{}", "a1".repeat(18));
        assert_eq!(check(&[ghp], &POLICY).left_out, ["a flag"]);
        assert_eq!(check(&words(&["--a b=1"]), &POLICY).left_out, ["a flag"]);
    }

    #[test]
    fn lost_sessions() {
        let ghp = format!("ghp_{}", "a1".repeat(18));
        let c = check(&words(&["--disallowedTools", "Edit", &format!("Bash({ghp})")]), &POLICY);
        assert_eq!(c.lost, Some(Lost::DenyRule));
        let c = check(&words(&["--allowedTools", "Edit", &format!("Bash({ghp})")]), &POLICY);
        assert_eq!((c.lost, c.kept), (None, words(&["--allowedTools", "Edit"])));
        assert_eq!(
            check(&words(&["--model", "o3", "mcp", "list"]), &POLICY).lost,
            Some(Lost::NotASession("the `mcp` command".into()))
        );
        // Only as the first word: a later word is a prompt.
        assert_eq!(check(&words(&["fix", "mcp"]), &POLICY).lost, None);
        assert_eq!(check(&words(&["--print"]), &POLICY).lost, Some(Lost::NotASession("a launch with --print".into())));
        assert_eq!(
            check(&words(&["--print=1"]), &POLICY).lost_reason().unwrap(),
            "a launch with --print is not a session that can be resumed"
        );
    }

    #[test]
    fn credentials_are_found_by_their_format() {
        // Built at runtime so secret scanners don't flag the source.
        let found = [
            format!("ghp_{}", "a1".repeat(18)),
            format!("x github_pat_{}", "a1".repeat(10)),
            format!("sk-ant-{}", "abcdefgh"),
            format!("sk-proj-{}", "abcdefgh"),
            format!("xoxb-{}", "1".repeat(10)),
            format!("glpat-{}", "abcdefgh"),
            format!("AKIA{}", "Q7".repeat(8)),
            format!("AIza{}", "S".repeat(35)),
            format!("eyJ{}.eyJ{}.{}", "hbGciOi", "zdWIiOiIx", "c2ln"),
            format!("postgres://u:{}@h/db", "hunter2"),
        ];
        for v in &found {
            assert!(holds_credential(v), "{v}");
        }
        for v in ["/w/sk-learn-2024", "sk_test_mode", "aizawa", "/w/akiapolaau", "https://user@example.com/x", "opus"] {
            assert!(!holds_credential(v), "{v}");
        }
    }
}
