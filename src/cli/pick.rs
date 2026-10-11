//! Interactive prompts. Questions go to stderr so stdout stays clean.

use std::io::{self, BufRead, Write};

use anyhow::Result;

use crate::adapters::store::fs::Store;
use crate::error::{Exit, failure};

fn read_line() -> Result<String> {
    let mut s = String::new();
    io::stdin().lock().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

pub fn confirm(question: &str) -> Result<bool> {
    eprint!("{question} [y/N] ");
    io::stderr().flush()?;
    Ok(matches!(read_line()?.to_lowercase().as_str(), "y" | "yes"))
}

pub fn ask(prompt: &str) -> Result<String> {
    eprint!("{prompt}: ");
    io::stderr().flush()?;
    read_line()
}

/// Shows a numbered list and returns the chosen index.
pub fn choose(title: &str, items: &[String]) -> Result<usize> {
    eprintln!("{title}");
    for (i, item) in items.iter().enumerate() {
        eprintln!("  {:>2}) {item}", i + 1);
    }
    let answer = ask("Number")?;
    match answer.parse::<usize>() {
        Ok(n) if (1..=items.len()).contains(&n) => Ok(n - 1),
        _ => Err(failure(Exit::Error, format!("'{answer}' is not a number from the list"))),
    }
}

pub fn choose_save_name(store: &Store) -> Result<String> {
    let mut items = store.list()?;
    let existing = items.len();
    items.push("(new snapshot)".into());
    let i = choose("Save to which snapshot?", &items)?;
    if i < existing { Ok(items[i].clone()) } else { ask("New snapshot name") }
}

pub fn choose_snapshot(store: &Store) -> Result<String> {
    let items = store.list()?;
    if items.is_empty() {
        return Err(failure(Exit::NotFound, "no snapshots yet"));
    }
    let i = choose("Restore which snapshot?", &items)?;
    Ok(items[i].clone())
}
