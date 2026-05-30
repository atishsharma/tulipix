//! `tulipix` CLI — thin front-end over `tulipix-tools`. Parses argv with the
//! shared parser so every GUI Tools op is reachable as a subcommand, sharing
//! the same Rust core and `tools.db` queue.

use anyhow::Result;
use tulipix_tools::cli::{self, ParseError};
use tulipix_tools::section;

fn main() -> Result<()> {
    tracing_subscriber::fmt().init();
    let args: Vec<String> = std::env::args().skip(1).collect();

    match cli::parse(&args) {
        Ok(cmd) => {
            let category = section::category_of(&cmd.subcommand);
            if cmd.json {
                println!("{}", serde_json::to_string(&cmd)?);
            } else {
                println!(
                    "tulipix {} [{}]{}{}",
                    cmd.subcommand,
                    category.label(),
                    if cmd.dry_run { " (dry-run)" } else { "" },
                    if cmd.queue { " (queued)" } else { "" },
                );
            }
            Ok(())
        }
        Err(ParseError::NoSubcommand) => {
            println!("tulipix — Tools CLI. Subcommands:");
            for s in cli::SUBCOMMANDS {
                println!("  {s}");
            }
            println!("\nShared flags: --json  --dry-run  --queue");
            Ok(())
        }
        Err(ParseError::UnknownSubcommand(s)) => {
            eprintln!("unknown subcommand: {s}\nrun `tulipix` with no args to list subcommands");
            std::process::exit(2);
        }
    }
}
