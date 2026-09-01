//! `np.p4.tools.cli` — `tulipix` CLI parity layer.
//!
//! Every Tools op is a CLI subcommand sharing this crate's logic and `tools.db`
//! queue with the GUI. This parses argv into a [`ParsedCommand`] with the
//! shared `--json` / `--dry-run` / `--queue` flags; the binary dispatches on it.
//!
//! The list used to be hand-maintained here in kebab-case, and had drifted: it
//! offered `metadata` and `pdf`, neither of which `exec::plan` has an arm for,
//! and it never translated `compress-video` to the `compress_video` the rest of
//! the crate matches on. Both are fixed by deriving from [`crate::catalog`].

use serde::{Deserialize, Serialize};

/// The one subcommand that is not an operation.
pub const QUEUE: &str = "queue";

/// Every subcommand the CLI exposes: one per catalogue op, plus `queue`.
pub fn subcommands() -> impl Iterator<Item = &'static str> {
    crate::catalog::kinds().chain(std::iter::once(QUEUE))
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParsedCommand {
    pub subcommand: String,
    /// `--json` machine-readable output.
    pub json: bool,
    /// `--dry-run` plan only, no side effects.
    pub dry_run: bool,
    /// `--queue` submit to the shared job queue instead of running inline.
    pub queue: bool,
    /// Remaining positional args (paths, etc.).
    pub positionals: Vec<String>,
    /// `--key value` / `--key=value` options.
    pub options: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    NoSubcommand,
    UnknownSubcommand(String),
}

/// Parse argv (without the program name) into a command.
pub fn parse(args: &[String]) -> Result<ParsedCommand, ParseError> {
    let mut it = args.iter();
    let raw = it.next().ok_or(ParseError::NoSubcommand)?;
    // Resolved, not just validated: `compress-video` becomes `compress_video`
    // here so everything downstream sees the one canonical spelling.
    let sub = if raw.as_str() == QUEUE {
        QUEUE
    } else {
        crate::catalog::resolve(raw).ok_or_else(|| ParseError::UnknownSubcommand(raw.clone()))?
    };
    let mut cmd = ParsedCommand { subcommand: sub.to_string(), ..Default::default() };
    let mut pending: Option<String> = None;
    for arg in it {
        if let Some(key) = pending.take() {
            cmd.options.push((key, arg.clone()));
            continue;
        }
        match arg.as_str() {
            "--json" => cmd.json = true,
            "--dry-run" => cmd.dry_run = true,
            "--queue" => cmd.queue = true,
            a if a.starts_with("--") => {
                let body = &a[2..];
                match body.split_once('=') {
                    Some((k, v)) => cmd.options.push((k.to_string(), v.to_string())),
                    None => pending = Some(body.to_string()), // value comes next
                }
            }
            other => cmd.positionals.push(other.to_string()),
        }
    }
    if let Some(flagless) = pending { cmd.options.push((flagless, String::new())); }
    Ok(cmd)
}

impl ParsedCommand {
    pub fn option(&self, key: &str) -> Option<&str> {
        self.options.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &[&str]) -> Vec<String> { s.iter().map(|x| x.to_string()).collect() }

    #[test]
    fn parses_subcommand_flags_and_options() {
        let cmd = parse(&argv(&["convert", "in.mov", "--to=mp4", "--crf", "23", "--json", "--queue"])).unwrap();
        assert_eq!(cmd.subcommand, "convert");
        assert!(cmd.json && cmd.queue && !cmd.dry_run);
        assert_eq!(cmd.positionals, vec!["in.mov"]);
        assert_eq!(cmd.option("to"), Some("mp4"));
        assert_eq!(cmd.option("crf"), Some("23"));
    }

    #[test]
    fn rejects_unknown_and_empty() {
        assert_eq!(parse(&argv(&[])), Err(ParseError::NoSubcommand));
        assert_eq!(parse(&argv(&["frobnicate"])), Err(ParseError::UnknownSubcommand("frobnicate".into())));
        // Both were listed as subcommands and neither has an `exec::plan` arm.
        assert!(parse(&argv(&["metadata"])).is_err());
        assert!(parse(&argv(&["pdf"])).is_err());
    }

    #[test]
    fn kebab_resolves_to_the_canonical_kind() {
        let cmd = parse(&argv(&["compress-video", "in.mov"])).unwrap();
        assert_eq!(cmd.subcommand, "compress_video");
        assert_eq!(parse(&argv(&["folder-diff"])).unwrap().subcommand, "folder_diff");
    }

    #[test]
    fn queue_is_a_subcommand_but_not_an_op() {
        assert_eq!(parse(&argv(&["queue"])).unwrap().subcommand, "queue");
        assert!(subcommands().any(|s| s == "queue"));
        assert!(crate::catalog::get("queue").is_none());
    }

    #[test]
    fn dry_run_flag() {
        let cmd = parse(&argv(&["rename", "--dry-run", "*.jpg"])).unwrap();
        assert!(cmd.dry_run);
        assert_eq!(cmd.positionals, vec!["*.jpg"]);
    }
}
