//! Which saved launch flags are kept. Each agent's policy (in the agent registry,
//! `adapters::agents::registry`) lists every flag its command line knows, with its arity, and marks
//! the few that are saved; every other flag is left out. A kept flag's values are still checked: by
//! their kind, and for the few credential formats that can be told apart for certain.
//!
//! The parser never guesses: a flag outside the table (its arity unknown, so whatever follows is
//! ambiguous), or a flag that limits the agent and can't be kept exactly as given, leaves out the
//! whole session. Flag arities and the launches that can't be resumed follow cmux's
//! `AgentLaunchSanitizer` policies, checked against `claude --help` and `codex --help`.

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
    /// Permission rules (`Bash(git:*)`, `Edit`), not empty.
    Rules,
    /// `key=value`: kept for `keys` and `restricting` keys, the value `Word`-shaped (optionally in
    /// double quotes). An entry ending in `.` or `_` stands for every key starting with it
    /// (`features.`, `model_`). A `restricting` key limits the agent: one that can't be saved
    /// leaves out the session. A `neutral` key is left out with a note; a key on no list leaves out
    /// the session, since it may limit the agent.
    ConfigKey { keys: &'static [&'static str], restricting: &'static [&'static str], neutral: &'static [&'static str] },
    /// Not restored, but known to take a value (or an optional one): the value goes with the flag,
    /// and is never read as a prompt, a subcommand or another flag.
    Skip,
    /// Not restored, and takes no value.
    Dropped,
}

impl Value {
    /// The flag never takes the next word as its value.
    fn is_switch(self) -> bool {
        matches!(self, Value::Switch | Value::Dropped)
    }
}

/// One flag an agent knows: its spellings, what it takes and whether it is restored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flag {
    pub names: &'static [&'static str],
    pub value: Value,
    /// Takes one or more values (`--add-dir a b`).
    pub many: bool,
    /// Limits what the agent may do (a deny rule, a sandbox, a permission mode): when it, or any
    /// of its values, can't be kept, the whole session is left out rather than resumed with fewer
    /// limits.
    pub restricts: bool,
    /// Its value is optional (`--resume [id]`): it takes the next word only when that is not a flag.
    pub optional: bool,
    /// Left out without a note: it picks the session, which the resume command gives anyway.
    pub silent: bool,
}

impl Flag {
    pub const fn switch(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Switch, many: false, restricts: false, optional: false, silent: false }
    }
    pub const fn word(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Word, many: false, restricts: false, optional: false, silent: false }
    }
    pub const fn words(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Word, many: true, restricts: false, optional: false, silent: false }
    }
    pub const fn path(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Path, many: false, restricts: false, optional: false, silent: false }
    }
    pub const fn paths(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Path, many: true, restricts: false, optional: false, silent: false }
    }
    pub const fn rules(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Rules, many: true, restricts: false, optional: false, silent: false }
    }
    pub const fn config(
        names: &'static [&'static str],
        keys: &'static [&'static str],
        restricting: &'static [&'static str],
        neutral: &'static [&'static str],
    ) -> Flag {
        Flag {
            names,
            value: Value::ConfigKey { keys, restricting, neutral },
            many: false,
            restricts: false,
            optional: false,
            silent: false,
        }
    }
    /// The same flag, marked as limiting the agent (see `restricts`).
    pub const fn restricting(self) -> Flag {
        Flag { restricts: true, ..self }
    }
    /// The same flag, with an optional value (see `optional`).
    pub const fn optional(self) -> Flag {
        Flag { optional: true, ..self }
    }
    /// The same flag, left out without a note (see `silent`).
    pub const fn silent(self) -> Flag {
        Flag { silent: true, ..self }
    }
    pub const fn skip(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Skip, many: false, restricts: false, optional: false, silent: false }
    }
    pub const fn skip_many(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Skip, many: true, restricts: false, optional: false, silent: false }
    }
    pub const fn dropped(names: &'static [&'static str]) -> Flag {
        Flag { names, value: Value::Dropped, many: false, restricts: false, optional: false, silent: false }
    }
}

/// An agent's flag policy.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// Every flag the agent's command line knows (all spellings, with arities); only those that are
    /// not `Value::Skip` or `Value::Dropped` are kept. A flag not listed loses the session.
    pub flags: &'static [Flag],
    /// Subcommands that are not a session (`claude mcp …`): such a launch is not resumed.
    pub commands: &'static [&'static str],
    /// Flags that make a launch one that can't be resumed (`--print`).
    pub rejects: &'static [&'static str],
    /// Nobody has tested sessionsnap with the agent's flags: a launch with any flag is not resumed
    /// (an unknown flag may limit the agent), only one without flags.
    pub untested: bool,
    /// The agent's parser gives a required value the next word whatever it starts with (commander,
    /// Claude Code), instead of reading a word that starts with `-` as a flag (clap, Codex).
    pub dash_values: bool,
}

impl Policy {
    /// The defaults: a clap-style parser. Build policies as
    /// `Policy { flags: …, ..Policy::BASE }`.
    pub const BASE: Policy = Policy { flags: &[], commands: &[], rejects: &[], untested: false, dash_values: false };
    /// Agents whose flags sessionsnap doesn't know: only launches without flags are resumed.
    pub const NONE: Policy = Policy { untested: true, ..Policy::BASE };
}

/// Why a whole agent session is left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lost {
    /// A flag that limits the agent (named here) could not be kept: resuming without it would allow
    /// more than the user did.
    Restriction(String),
    /// The agent's flags are untested, and the launch has some.
    UntestedFlags,
    /// A flag outside the agent's table (named here): what follows it can't be read for certain.
    UnknownFlag(String),
    /// A config flag (named first) set a key on no list (named second), which may limit the agent.
    UnlistedConfigKey(String, String),
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
    /// More to say in the note about what was left out (no values).
    pub remarks: Vec<String>,
}

impl FlagCheck {
    /// Why the session is left out, in words, when it is; `agent` names the agent.
    pub fn lost_reason(&self, agent: &str) -> Option<String> {
        self.lost.as_ref().map(|l| match l {
            Lost::Restriction(name) => {
                format!("a restricting flag ({name}) could not be saved; resume it by hand")
            }
            Lost::UntestedFlags => format!("sessionsnap can't restore {agent}'s launch flags yet; resume it by hand"),
            Lost::UnknownFlag(name) => format!("an unknown flag {name}; resume it by hand"),
            Lost::UnlistedConfigKey(flag, key) => {
                format!("an unlisted {agent} {flag} key {key}; resume it by hand")
            }
            Lost::NotASession(what) => format!("{what} is not a session that can be resumed"),
        })
    }

    /// What was left out, for `save`'s notes: flag names only, never a value.
    pub fn note(&self, agent: &str) -> Option<String> {
        if let Some(why) = self.lost_reason(agent) {
            return Some(format!("left out its agent session: {why}"));
        }
        let others = match self.other_words {
            0 => String::new(),
            1 => "1 other argument".to_string(),
            n => format!("{n} other arguments"),
        };
        let note = match (self.left_out.is_empty(), others.is_empty()) {
            (true, true) => None,
            (false, true) => Some(format!("left out flags it does not restore: {}", self.left_out.join(", "))),
            (false, false) => {
                Some(format!("left out flags it does not restore: {} (and {others})", self.left_out.join(", ")))
            }
            (true, false) => Some(format!("left out {others} it does not restore")),
        };
        note.map(|n| [n].into_iter().chain(self.remarks.iter().cloned()).collect::<Vec<_>>().join("; "))
    }
}

/// One flag as given: its flag, the name it was written with, its values, and how to write it
/// again (`whole`: the original word, when the flag and an inline value were one word of their own).
struct Occurrence<'a> {
    flag: &'static Flag,
    name: String,
    values: Vec<&'a str>,
    whole: Option<&'a str>,
}

/// `args` filtered through `policy`: listed flags whose values pass their checks are kept (in their
/// saved form, `--flag value`, `--flag=value` or `-svalue`; a flag from a cluster such as `-xs v`
/// as `-s v`); everything else is left out. The session is lost (see `Lost`) when a restricting
/// flag or value is not kept exactly as given, when a flag is not in the agent's table, when an
/// untested agent's launch has any flag, or for a subcommand in `policy.commands` or a flag in
/// `policy.rejects`.
pub fn check(args: &[String], policy: &Policy) -> FlagCheck {
    let mut out = FlagCheck::default();
    let find = |name: &str| policy.flags.iter().find(|f| f.names.contains(&name));
    let mut occurrences: Vec<Occurrence> = vec![];
    let mut seen_word = false;
    let mut i = 0;
    // Pass 1: read the words as the agent's parser would (clap and commander agree on these rules).
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if a == "--" {
            out.other_words += args.len() - i;
            break;
        }
        if !a.starts_with('-') || a == "-" {
            if !seen_word && policy.commands.contains(&a) {
                lose(&mut out, Lost::NotASession(format!("the `{a}` command")));
            }
            seen_word = true;
            out.other_words += 1;
            continue;
        }
        if policy.untested {
            lose(&mut out, Lost::UntestedFlags);
            note_name(&mut out, a.split('=').next().unwrap_or(a));
            continue;
        }
        // The flags in this word, each with the value written into the word, if any.
        let mut parts: Vec<(&'static Flag, String, Option<&str>)> = vec![];
        if a.starts_with("--") {
            let (name, inline) = match a.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (a, None),
            };
            if policy.rejects.contains(&name) {
                lose(&mut out, Lost::NotASession(format!("a launch with {name}")));
            }
            match find(name) {
                Some(f) => parts.push((f, name.to_string(), inline)),
                None => {
                    lose(&mut out, Lost::UnknownFlag(shown_name(name)));
                    note_name(&mut out, name);
                }
            }
        } else {
            // `-x`, `-xvalue`, `-x=value`, or a cluster of short flags `-abc` / `-abvalue`.
            let mut rest = &a[1..];
            while let Some(c) = rest.chars().next() {
                let name = format!("-{c}");
                rest = &rest[c.len_utf8()..];
                if policy.rejects.contains(&name.as_str()) {
                    lose(&mut out, Lost::NotASession(format!("a launch with {name}")));
                }
                let Some(f) = find(&name) else {
                    lose(&mut out, Lost::UnknownFlag(shown_name(&name)));
                    note_name(&mut out, &name);
                    break;
                };
                if f.value.is_switch() {
                    parts.push((f, name, None));
                    continue;
                }
                let attached = rest.strip_prefix('=').unwrap_or(rest);
                parts.push((f, name, (!rest.is_empty()).then_some(attached)));
                break;
            }
        }
        let alone = parts.len() == 1;
        let last = parts.len().saturating_sub(1);
        for (n, (flag, name, inline)) in parts.into_iter().enumerate() {
            let values: Vec<&str> = match inline {
                Some(v) => vec![v],
                None if flag.value.is_switch() || n != last => vec![],
                None => {
                    let start = i;
                    // A required value under commander: the next word, whatever it starts with.
                    if policy.dash_values && !flag.optional && i < args.len() {
                        i += 1;
                    }
                    while i < args.len() && !args[i].starts_with('-') && (flag.many || i == start) {
                        i += 1;
                    }
                    args[start..i].iter().map(String::as_str).collect()
                }
            };
            let whole = (alone && (inline.is_some() || flag.value.is_switch())).then_some(a);
            occurrences.push(Occurrence { flag, name, values, whole });
        }
    }
    // Pass 2: keep what is allowed and fits; every restricting flag must come out as it went in.
    for o in occurrences {
        if let Value::ConfigKey { keys, restricting, neutral } = o.flag.value {
            for v in &o.values {
                let key = config_key(v);
                if ![keys, restricting, neutral].iter().any(|list| key_in(list, key)) {
                    lose(&mut out, Lost::UnlistedConfigKey(o.name.clone(), shown_key(v)));
                }
            }
        }
        let restricts = o.flag.restricts || o.values.iter().any(|v| restricting_key(o.flag.value, v));
        let kept: Vec<&str> = match o.flag.value {
            Value::Skip | Value::Dropped | Value::Switch => vec![],
            kind => o.values.iter().copied().filter(|v| fits(kind, v) && !holds_credential(v)).collect(),
        };
        let keep_switch = o.flag.value == Value::Switch && o.values.is_empty();
        let whole = keep_switch || (!kept.is_empty() && kept.len() == o.values.len());
        if !whole {
            if !o.flag.silent {
                note_name(&mut out, &o.name);
            }
            if restricts {
                lose(&mut out, Lost::Restriction(o.name.clone()));
            }
        }
        if keep_switch {
            out.kept.push(o.whole.unwrap_or(&o.name).to_string());
        } else if !kept.is_empty() {
            match o.whole {
                Some(word) if whole => out.kept.push(word.to_string()),
                _ => {
                    out.kept.push(o.name.clone());
                    out.kept.extend(kept.iter().map(|v| v.to_string()));
                }
            }
        }
    }
    out
}

fn lose(out: &mut FlagCheck, lost: Lost) {
    out.lost.get_or_insert(lost);
}

/// How a flag's name is shown in a note: never anything that could be a value.
fn shown_name(name: &str) -> String {
    let mut out = FlagCheck::default();
    note_name(&mut out, name);
    out.left_out.pop().unwrap_or_default()
}

/// `v` is a `key=value` (or a bare key) of a restricting config key.
fn restricting_key(kind: Value, v: &str) -> bool {
    match kind {
        Value::ConfigKey { restricting, .. } => key_in(restricting, config_key(v)),
        _ => false,
    }
}

/// The key of a `key=value` config value (the whole value when it has no `=`).
fn config_key(v: &str) -> &str {
    v.split_once('=').map_or(v, |(k, _)| k).trim()
}

/// How a config value's key is shown in a note: only its first segment, as a plain name
/// (`tools.web_search` shows as `tools.*`), never anything that could be a value or a variable's
/// name; "(not shown)" when that can't be told (no `=`, quotes, spaces, a token).
fn shown_key(v: &str) -> String {
    let Some((key, _)) = v.split_once('=') else { return "(not shown)".to_string() };
    let key = key.trim();
    let first = key.split('.').next().unwrap_or("");
    let plain = !first.is_empty()
        && first.len() <= 64
        && key.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c))
        && !holds_credential(key);
    match (plain, first.len() < key.len()) {
        (false, _) => "(not shown)".to_string(),
        (true, true) => format!("{first}.*"),
        (true, false) => first.to_string(),
    }
}

/// `key` is one of `keys`, or starts with one ending in `.` or `_`.
fn key_in(keys: &[&str], key: &str) -> bool {
    keys.iter()
        .any(|k| *k == key || ((k.ends_with('.') || k.ends_with('_')) && key.starts_with(k) && key.len() > k.len()))
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
        Value::Switch | Value::Skip | Value::Dropped => false,
        Value::Word => is_word(v),
        Value::Path => {
            let t = v.trim_start();
            !t.is_empty() && !t.starts_with('{') && !t.starts_with('[')
        }
        // An empty rule (`--tools ""`, `--disallowedTools=`) can't be told from a lost value.
        Value::Rules => !v.is_empty(),
        Value::ConfigKey { keys, restricting, .. } => v.split_once('=').is_some_and(|(k, value)| {
            let unquoted = value.strip_prefix('"').and_then(|x| x.strip_suffix('"')).unwrap_or(value);
            (key_in(keys, k) || key_in(restricting, k)) && is_word(unquoted)
        }),
    }
}

fn is_word(v: &str) -> bool {
    !v.is_empty() && v.len() <= 100 && v.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:/@,".contains(c))
}

/// Prefixes specific to one token format, with how many characters must follow (Anthropic, OpenAI projects, GitHub, GitLab), taken as a
/// token when 8 or more characters follow. Slack's `xox?-` is checked in `holds_credential`.
const TOKEN_PREFIXES: &[(&str, usize)] = &[
    ("sk-ant-", 8),
    ("sk-proj-", 8),
    ("sk-svcacct-", 8),
    ("sk-admin-", 8),
    ("sk_live_", 16),
    ("rk_live_", 16),
    ("ghp_", 8),
    ("gho_", 8),
    ("ghu_", 8),
    ("ghs_", 8),
    ("ghr_", 8),
    ("github_pat_", 8),
    ("glpat-", 8),
    ("xapp-", 8),
    // Short prefixes ordinary names start with too: only with a token's length after them.
    ("hf_", 30),
    ("npm_", 30),
];

/// `v` holds, anywhere, a credential in a format that can be told apart for certain: a token with
/// a specific prefix (see `TOKEN_PREFIXES`, Slack `xox?-`, AWS `AKIA`/`ASIA` plus 16, Google
/// `AIza` plus 35), a JWT, a URL with a password (`scheme://user:pass@host`), or an HTTP credential
/// (see `auth_header`).
pub fn holds_credential(v: &str) -> bool {
    token_in(v) || url_with_password(v) || auth_header(v)
}

/// `v` holds an HTTP credential: `Bearer <token>` (the token at least 12 characters), or a value
/// after an `Authorization:`, `X-Api-Key:` or `Api-Key:` header (after one scheme word such as
/// `Basic`, at least 4 characters), any case; or curl's `-u`/`--user` with `user:password`, also
/// attached (`-uuser:password`, `--user=user:password`).
fn auth_header(v: &str) -> bool {
    let lower = v.to_ascii_lowercase();
    let token = |s: &str| s.chars().take_while(|c| c.is_ascii_alphanumeric() || "._~+/=-".contains(*c)).count();
    let bearer = lower.match_indices("bearer ").any(|(i, m)| token(lower[i + m.len()..].trim_start()) >= 12);
    let header = ["authorization:", "api-key:"].iter().any(|h| {
        lower.match_indices(h).any(|(i, m)| {
            let rest = lower[i + m.len()..].trim_start();
            // One scheme word (`Basic `, `Bearer `, `token `) before the value.
            let scheme = rest.chars().take_while(char::is_ascii_alphabetic).count();
            let after = match rest[scheme..].strip_prefix(' ') {
                Some(after) if scheme > 0 => after.trim_start(),
                _ => rest,
            };
            token(after) >= 4
        })
    });
    // Any value with a password after its `:` (a numeric `uid:gid` too: it can't be told from a
    // numeric password, so `docker -u 1000:1000` is dropped as well, failing safe).
    let user_pass = |w: &str| w.split_once(':').is_some_and(|(_, p)| !p.is_empty());
    let curl_user = |words: &[String]| {
        words.iter().enumerate().any(|(i, w)| {
            (matches!(w.as_str(), "-u" | "--user") && words.get(i + 1).is_some_and(|n| user_pass(n)))
                || w.strip_prefix("--user=").is_some_and(user_pass)
                // The attached short form, `-uuser:password`.
                || w.strip_prefix("-u").is_some_and(|rest| !rest.starts_with('-') && user_pass(rest))
        })
    };
    // Found in any reading of the words counts (see `readings`).
    let curl_user = readings(v).iter().any(|words| curl_user(words));
    bearer || header || curl_user
}

/// Several ways to split `v` into words; a check that finds a credential in any of them drops the
/// value, so no one reading can hide it:
/// - split on whitespace, quotes and parentheses (the `Bash(…)` of a rule);
/// - the way a shell joins them (`admin:"pw"` is one word `admin:pw`), where a word ending in `:`
///   also takes the quoted word after it (`-u 'admin:' "pw"`);
/// - the readings of the text inside each quoted part (`sh -c 'curl -u …'`).
///
/// A quote with no closing one is only a separator, never the start of one long word.
fn readings(v: &str) -> Vec<Vec<String>> {
    let plain =
        v.split(|c: char| c.is_whitespace() || "\"'()".contains(c)).filter(|w| !w.is_empty()).map(str::to_string);
    let mut out = vec![plain.collect()];
    let mut words: Vec<(String, bool)> = vec![];
    let mut inner: Vec<String> = vec![];
    let (mut word, mut started, mut quoted) = (String::new(), false, false);
    let chars: Vec<char> = v.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let close = if c == '\'' || c == '"' { chars[i + 1..].iter().position(|&q| q == c) } else { None };
        match close {
            Some(n) => {
                let part: String = chars[i + 1..i + 1 + n].iter().collect();
                if !started {
                    quoted = true;
                }
                started = true;
                word.push_str(&part);
                inner.push(part);
                i += n + 2;
                continue;
            }
            None if c.is_whitespace() || "\"'()".contains(c) => {
                if started {
                    words.push((std::mem::take(&mut word), quoted));
                }
                (started, quoted) = (false, false);
            }
            None => {
                started = true;
                word.push(c);
            }
        }
        i += 1;
    }
    if started {
        words.push((word, quoted));
    }
    let mut joined: Vec<String> = vec![];
    for (w, quoted) in words {
        match joined.last_mut() {
            Some(last) if quoted && last.ends_with(':') => last.push_str(&w),
            _ => joined.push(w),
        }
    }
    out.push(joined);
    // Each quoted part is shorter than `v`, so this ends.
    for part in inner {
        out.extend(readings(&part));
    }
    out
}

fn token_in(v: &str) -> bool {
    v.split(|c: char| !(c.is_ascii_alphanumeric() || "_-.".contains(c))).any(|word| {
        // A word may run on into a dot (a sentence's end); prefixes hold none.
        let plain = word.split('.').next().unwrap_or("");
        let lower = plain.to_ascii_lowercase();
        let b = plain.as_bytes();
        // A prefix counts at the word's start or after a `-` or `_` in it (`opus-ghp_…`).
        let prefixed = TOKEN_PREFIXES.iter().any(|(p, min)| {
            lower.match_indices(p).any(|(i, _)| {
                (i == 0 || !lower.as_bytes()[i - 1].is_ascii_alphanumeric()) && lower.len() >= i + p.len() + min
            })
        });
        let slack = b.len() >= 13 && lower.starts_with("xox") && b[3].is_ascii_alphabetic() && b[4] == b'-';
        let aws = (plain.starts_with("AKIA") || plain.starts_with("ASIA"))
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
            Flag::rules(&["--disallowedTools"]).restricting(),
            Flag::word(&["--sandbox", "-s"]).restricting(),
            Flag::switch(&["--strict"]).restricting(),
            Flag::config(&["-c"], &["model"], &["sandbox_mode", "features."], &["model_", "tui."]),
            Flag::skip(&["--name"]),
            Flag::dropped(&["--print"]),
            Flag::dropped(&["-v"]),
        ],
        commands: &["mcp"],
        rejects: &["--print"],
        ..Policy::BASE
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
        let c = check(&words(&["--name=abc", "--model", "a b", "y"]), &POLICY);
        assert_eq!(c.left_out, ["--name", "--model"]);
        assert_eq!(c.note("x").unwrap(), "left out flags it does not restore: --name, --model (and 1 other argument)");
        // Unknown flags lose the session, and are named the same way.
        let c = check(&words(&["--token=abc", "-kSECRET", "--x", "y"]), &POLICY);
        assert_eq!(c.left_out, ["--token", "-k", "--x"]);
        assert_eq!(c.note("x").unwrap(), "left out its agent session: an unknown flag --token; resume it by hand");
        let ghp = format!("--ghp_{}", "a1".repeat(18));
        assert_eq!(check(&[ghp], &POLICY).left_out, ["a flag"]);
        assert_eq!(check(&words(&["--a b=1"]), &POLICY).left_out, ["a flag"]);
    }

    #[test]
    fn lost_sessions() {
        let ghp = format!("ghp_{}", "a1".repeat(18));
        let c = check(&words(&["--disallowedTools", "Edit", &format!("Bash({ghp})")]), &POLICY);
        assert_eq!(c.lost, Some(Lost::Restriction("--disallowedTools".into())));
        assert_eq!(
            c.note("Claude Code").unwrap(),
            "left out its agent session: a restricting flag (--disallowedTools) could not be saved; resume it by hand"
        );
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
            check(&words(&["--print=1"]), &POLICY).lost_reason("x").unwrap(),
            "a launch with --print is not a session that can be resumed"
        );
    }

    /// A restricting flag that can't be kept, for any reason, loses the session.
    #[test]
    fn restricting_flags_fail_closed() {
        let ghp = format!("ghp_{}", "a1".repeat(18));
        for (args, name) in [
            (words(&["--sandbox", "read only"]), "--sandbox"),
            (words(&["-s", &ghp]), "-s"),
            (words(&["--sandbox"]), "--sandbox"),
            (words(&["-s", "--model", "o3"]), "-s"),
            (words(&["--sandbox={}"]), "--sandbox"),
            (words(&["--strict=1"]), "--strict"),
            (words(&["-c", "sandbox_mode=[\"x\"]"]), "-c"),
            (words(&["-c", "sandbox_mode"]), "-c"),
            (words(&["-c=features.web_search={a=1}"]), "-c"),
            (words(&["--disallowedTools", "Edit", &format!("Bash(x {ghp})")]), "--disallowedTools"),
        ] {
            assert_eq!(check(&args, &POLICY).lost, Some(Lost::Restriction(name.into())), "{args:?}");
        }
        // Kept when they fit, in every form.
        for args in [
            words(&["--sandbox", "read-only"]),
            words(&["-sread-only"]),
            words(&["-s=read-only"]),
            words(&["--strict"]),
            words(&["-c", "sandbox_mode=read-only"]),
            words(&["-c", "features.web_search=false"]),
        ] {
            assert_eq!(check(&args, &POLICY), FlagCheck { kept: args.clone(), ..Default::default() }, "{args:?}");
        }
        // Neutral flags and config keys are only noted.
        for args in [words(&["--model", "a b"]), words(&["-c", "model_x={}"]), words(&["--add-dir", "[x]"])] {
            let c = check(&args, &POLICY);
            assert_eq!((c.lost, c.left_out.len()), (None, 1), "{args:?}");
        }
    }

    /// The parser reads words as clap and commander do, and never guesses.
    #[test]
    fn reads_words_as_the_agent_does() {
        // A cluster of known shorts: `-v` is left out, `-s read-only` kept.
        let c = check(&words(&["-vs", "read-only"]), &POLICY);
        assert_eq!((c.kept, c.left_out, c.lost), (words(&["-s", "read-only"]), words(&["-v"]), None));
        // `-vsread-only` and `-vs=read-only` alike.
        for w in ["-vsread-only", "-vs=read-only"] {
            assert_eq!(check(&words(&[w]), &POLICY).kept, words(&["-s", "read-only"]), "{w}");
        }
        // An unknown letter: what follows can't be read for certain.
        let c = check(&words(&["-xs", "read-only"]), &POLICY);
        assert_eq!(c.lost, Some(Lost::UnknownFlag("-x".into())));
        // An unknown long flag, whatever its value.
        let c = check(&words(&["--new-flag", "value", "-s", "read-only"]), &POLICY);
        assert_eq!(c.lost, Some(Lost::UnknownFlag("--new-flag".into())));
        // A known flag that is left out takes its value with it: the rule after it is kept.
        let c = check(&words(&["--name", "foo", "--disallowedTools", "X"]), &POLICY);
        assert_eq!((c.kept, c.lost), (words(&["--disallowedTools", "X"]), None));
        // After `--` nothing is a flag.
        let c = check(&words(&["-m", "o3", "--", "--disallowedTools", "X", "--print", "--x"]), &POLICY);
        assert_eq!((c.kept, c.lost, c.other_words), (words(&["-m", "o3"]), None, 4));
        // An empty inline value of a restricting flag.
        for w in ["--sandbox=", "-s=", "--disallowedTools="] {
            assert!(matches!(check(&words(&[w]), &POLICY).lost, Some(Lost::Restriction(_))), "{w}");
        }
    }

    /// Every way a restricting flag can fail to come out as it went in loses the session.
    #[test]
    fn every_dropped_restriction_is_lost() {
        const P: Policy = Policy {
            flags: &[
                Flag::word(&["--mode", "-o"]).restricting(),
                Flag::path(&["--settings"]).restricting(),
                Flag::rules(&["--deny"]).restricting(),
                Flag::switch(&["--strict"]).restricting(),
                Flag::config(&["-c"], &["model"], &["sandbox_mode"], &[]),
                Flag::skip(&["--limit"]).restricting(),
                Flag::dropped(&["--locked"]).restricting(),
                Flag::switch(&["--verbose", "-V"]),
            ],
            commands: &[],
            rejects: &[],
            ..Policy::BASE
        };
        let ghp = format!("ghp_{}", "a1".repeat(18));
        for args in [
            words(&["--limit", "5"]),
            words(&["--locked"]),
            // A value that starts with `-`: no value, for every kind.
            words(&["--mode", "--verbose"]),
            words(&["--settings", "-V"]),
            words(&["--deny", "--verbose"]),
            words(&["-o"]),
            // Some values of a many-valued flag pass, some don't.
            words(&["--deny", "Edit", &format!("Bash({ghp})")]),
            // A restricting config key with a value that doesn't fit.
            words(&["-c", "sandbox_mode=a b"]),
            words(&["-c=sandbox_mode="]),
            // A switch written with a value.
            words(&["--strict=false"]),
            // The second of two copies.
            words(&["--mode", "plan", "--mode", "two words"]),
            // Inline JSON for a restricting path.
            words(&["--settings={}"]),
            // Inside a cluster.
            words(&["-Vo"]),
        ] {
            assert!(matches!(check(&args, &P).lost, Some(Lost::Restriction(_))), "{args:?}: {:?}", check(&args, &P));
        }
        // Kept as given, they keep the session.
        for args in [
            words(&["--mode", "plan", "--mode=auto", "-oplan", "-Vo", "plan"]),
            words(&["--deny", "Edit", "Bash(rm:*)", "--strict", "-c", "sandbox_mode=read-only"]),
        ] {
            let c = check(&args, &P);
            assert_eq!(c.lost, None, "{args:?}");
        }
    }

    /// A `-c` key on no list loses the session (it may limit the agent); a neutral key is only
    /// noted.
    #[test]
    fn unlisted_config_keys_lose_the_session() {
        for v in ["tools.web_search=false", "sandbox_permissions=[\"x\"]", "noequals", "", "mcp_servers.x.env={T=1}"] {
            let c = check(&words(&["-c", v]), &POLICY);
            assert!(matches!(c.lost, Some(Lost::UnlistedConfigKey(..))), "{v}: {c:?}");
            assert!(c.note("Codex").unwrap().contains("an unlisted Codex -c key"), "{v}");
        }
        let c = check(&words(&["-c", "tools.web_search=false"]), &POLICY);
        assert_eq!(c.lost_reason("Codex").unwrap(), "an unlisted Codex -c key tools.*; resume it by hand");
        // A key that could be a value is not shown.
        let ghp = format!("ghp_{}", "a1".repeat(18));
        assert!(!check(&words(&["-c", &format!("{ghp}=1")]), &POLICY).note("Codex").unwrap().contains("ghp_"));
        for v in ["model_context_window=1000", "tui.theme=dark", "model_x=a b"] {
            let c = check(&words(&["-c", v]), &POLICY);
            assert_eq!((c.lost, c.left_out), (None, words(&["-c"])), "{v}");
        }
    }

    /// An untested agent resumes only a launch without flags.
    #[test]
    fn untested_agents_resume_only_launches_without_flags() {
        let c = check(&words(&["--sandbox", "x"]), &Policy::NONE);
        assert_eq!(c.lost, Some(Lost::UntestedFlags));
        assert_eq!(
            c.note("Gemini CLI").unwrap(),
            "left out its agent session: sessionsnap can't restore Gemini CLI's launch flags yet; resume it by hand"
        );
        let c = check(&words(&["fix it"]), &Policy::NONE);
        assert_eq!((c.lost, c.kept, c.other_words), (None, vec![], 1));
        assert_eq!(check(&[], &Policy::NONE), FlagCheck::default());
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
            format!("sk-svcacct-{}", "Ab9".repeat(10)),
            format!("sk-admin-{}", "Ab9".repeat(10)),
            format!("sk_live_{}", "Ab9".repeat(8)),
            format!("rk_live_{}", "Ab9".repeat(8)),
            format!("hf_{}", "Ab9".repeat(12)),
            format!("npm_{}", "Ab9".repeat(12)),
            format!("ASIA{}", "Q7".repeat(8)),
            format!("xapp-1-{}", "A9".repeat(10)),
            format!("Bash(curl -H 'Authorization: Bearer {}' https://x)", "q7Zr".repeat(8)),
            format!("curl -H 'authorization: token {}'", "q7Zr".repeat(3)),
            format!("bearer {}", "q7Zr".repeat(3)),
            format!("Bash(curl -H \"Authorization: Basic {}\")", "YTpi"),
            format!("authorization:{}", "q7Zr9"),
            format!("Bash(curl -H 'X-Api-Key: {}' https://x)", "q7Zr9"),
            format!("api-key: {}", "q7Zr9"),
            format!("Bash(curl -u admin:{} https://x)", "pw12"),
            format!("Bash(curl --user=admin:{} https://x)", "pw12"),
            // The attached forms.
            format!("Bash(curl -uadmin:{} https://x)", "pw12"),
            format!("curl -u'admin:{}' https://x", "pw12"),
            format!("Bash(curl --user admin:{} https://x)", "pw12"),
            // Numeric passwords, and passwords in quotes.
            format!("Bash(curl -u 1234:{} https://x)", "567890"),
            format!("curl --user=4242:{}", "20240101"),
            format!("curl -uadmin:\"{}\" https://x", "s3cret"),
            format!("curl -u 'admin:' \"{}\" https://x", "pw"),
            format!("Bash(curl -u :{} https://x)", "pw12"),
            // Inside a quoted command, after an unclosed quote, in a quoted `-c` script.
            format!("Bash(sh -c 'curl -u admin:{} https://x')", "pw"),
            format!("Bash(echo don't && curl -u admin:{} https://x)", "pw"),
            format!("bash -lc \"curl --user=a:{} x\"", "b"),
            format!("Bash(sh -c 'curl -u \"admin:\" \"{}\"')", "pw"),
            // Fails safe: a numeric uid:gid can't be told from a numeric password.
            "Bash(docker run -u 1000:1000 img)".to_string(),
        ];
        for v in &found {
            assert!(holds_credential(v), "{v}");
        }
        for v in [
            "/w/sk-learn-2024",
            "sk_test_mode",
            "aizawa",
            "/w/akiapolaau",
            "https://user@example.com/x",
            "opus",
            "hf_model_name",
            "npm_scripts",
            "Bash(gh auth:*)",
            "bearer",
            "the bearer of news",
            "authorization: ",
            "api-key",
            "Bash(git push -u origin main)",
            "Bash(npm i -u)",
            // A user without a password.
            "Bash(curl -u admin https://x)",
            "Bash(curl -u 'admin' https://x)",
            "Bash(curl -uadmin: https://x)",
            "Bash(docker run -u 1000 img)",
            "Bash(git log:*)",
            "Bash(sh -c 'git push -u origin main')",
            "Bash(echo don't)",
        ] {
            assert!(!holds_credential(v), "{v}");
        }
    }
}
