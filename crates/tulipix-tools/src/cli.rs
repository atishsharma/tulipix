//! `np.p4.tools.cli` — `tulipix` CLI parity layer.
//!
//! Every Tools op is a CLI subcommand (rename/merge/split/compress/convert/
//! trim/extract/transcribe/…) sharing this crate's logic and `tools.db` queue
//! with the GUI. This parses argv into a [`ParsedCommand`] with the shared
//! `--json` / `--dry-run` / `--queue` flags; the binary dispatches on it.

use serde::{Deserialize, Serialize};

/// Every subcommand the CLI exposes (kebab-case, matches GUI op kinds).
pub const SUBCOMMANDS: &[&str] = &[
    "rename", "merge", "split", "compress-video", "compress-audio", "compress-photo",
    "convert", "trim", "extract", "metadata", "thumbnail", "watermark", "normalize",
    "transcribe", "download", "burn-subs", "resize", "pdf", "hash", "folder-diff", "queue",
];

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
    let sub = it.next().ok_or(ParseError::NoSubcommand)?.clone();
    if !SUBCOMMANDS.contains(&sub.as_str()) {
        return Err(ParseError::UnknownSubcommand(sub));
    }
    let mut cmd = ParsedCommand { subcommand: sub, ..Default::default() };
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
    }

    #[test]
    fn dry_run_flag() {
        let cmd = parse(&argv(&["rename", "--dry-run", "*.jpg"])).unwrap();
        assert!(cmd.dry_run);
        assert_eq!(cmd.positionals, vec!["*.jpg"]);
    }
}
