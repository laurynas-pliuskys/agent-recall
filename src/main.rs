use agent_recall::{cli, mcp};

use anyhow::Result;
use clap::{CommandFactory, Parser};

#[derive(Parser)]
#[command(name = "agent-recall")]
#[command(version)]
#[command(about = "Search conversation history with agent-recall MCP server")]
struct Cli {
    /// Verbosity level (-v for WARN, -vv for INFO, -vvv for DEBUG)
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,

    #[command(subcommand)]
    command: Option<cli::CliCommands>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Set up panic hook to handle broken pipe errors gracefully
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let panic_message = format!("{panic_info}");
        if panic_message.contains("Broken pipe") {
            // Silently exit when pipe is broken (e.g., piped to head, less, etc.)
            std::process::exit(0);
        }
        default_panic(panic_info);
    }));

    let args = Cli::parse();

    match args.command {
        Some(cli::CliCommands::Completions { shell }) => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "agent-recall",
                &mut std::io::stdout(),
            );
            Ok(())
        }
        Some(cli::CliCommands::Mcp) | None => {
            // Default to MCP server mode when no subcommand provided
            mcp::run_mcp_server().await
        }
        Some(command) => cli::run_cli(args.verbose, command),
    }
}
