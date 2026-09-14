//! `tulipix` CLI — thin front-end over `tulipix-tools`. Parses argv with the
//! shared parser so every GUI Tools op is reachable as a subcommand, sharing
//! the same Rust core and `tools.db` queue.
//!
//! One subcommand is not a Tools op: `mcp` serves the library to an outside
//! AI agent over stdio (see `mcp.rs`). It is handled before the parser,
//! because it owns stdout for the whole of its run.

use anyhow::Result;
use tulipix_tools::cli::{self, ParseError};
use tulipix_tools::section;

mod mcp;

fn main() -> Result<()> {
    // The MCP server talks JSON-RPC on stdout, so nothing else may: a log line
    // in the middle of the stream is a parse error at the other end. Its logs
    // go to stderr, which is where the agent's client shows them.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("mcp") {
        tracing_subscriber::fmt().with_writer(std::io::stderr).init();
        return tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(mcp::serve());
    }

    tracing_subscriber::fmt().init();

    match cli::parse(&args) {
        Ok(cmd) => {
            // `queue` is a subcommand without a category — it is the queue,
            // not an operation.
            let category = section::category_of(&cmd.subcommand)
                .map(|c| c.label())
                .unwrap_or("Queue");
            if cmd.json {
                println!("{}", serde_json::to_string(&cmd)?);
            } else {
                println!(
                    "tulipix {} [{}]{}{}",
                    cmd.subcommand,
                    category,
                    if cmd.dry_run { " (dry-run)" } else { "" },
                    if cmd.queue { " (queued)" } else { "" },
                );
            }
            Ok(())
        }
        Err(ParseError::NoSubcommand) => {
            println!("tulipix — Tools CLI. Subcommands:");
            for s in cli::subcommands() {
                println!("  {s}");
            }
            println!("  mcp   (serve the library to an AI agent over stdio)");
            println!("\nShared flags: --json  --dry-run  --queue");
            Ok(())
        }
        Err(ParseError::UnknownSubcommand(s)) => {
            eprintln!("unknown subcommand: {s}\nrun `tulipix` with no args to list subcommands");
            std::process::exit(2);
        }
    }
}
