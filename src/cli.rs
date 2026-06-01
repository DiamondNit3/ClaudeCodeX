use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "ccx")]
#[command(about = "Terminal-only agentic coding harness")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    #[command(about = "Start the interactive terminal harness")]
    Interactive,
    #[command(about = "Run one task non-interactively")]
    Exec {
        #[arg(required = true)]
        task: String,
    },
    #[command(about = "List resumable sessions or inspect one session id")]
    Resume { session: Option<String> },
    #[command(about = "Inspect or initialize configuration")]
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    #[command(about = "List configured providers")]
    Providers,
    #[command(about = "Inspect configured MCP servers")]
    Mcp {
        #[command(subcommand)]
        command: McpCommand,
    },
    #[command(about = "Review current git diff or selected paths")]
    Review {
        #[arg()]
        paths: Vec<std::path::PathBuf>,
    },
    #[command(about = "List reusable workflow skills")]
    Skills,
    #[command(about = "Manage background tasks")]
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    #[command(about = "Run a bounded model-backed helper subagent")]
    Subagent {
        kind: String,
        #[arg(required = true)]
        task: String,
    },
    #[command(about = "Run benchmark smoke checks")]
    Bench,
    #[command(about = "Print release checklist")]
    ReleaseCheck,
    #[command(about = "Print environment and configuration diagnostics")]
    Doctor,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    Init,
    Path,
    Show,
}

#[derive(Debug, Subcommand)]
pub enum McpCommand {
    List,
}

#[derive(Debug, Subcommand)]
pub enum TaskCommand {
    Spawn {
        #[arg(required = true)]
        task: String,
    },
    List,
    Show {
        id: String,
    },
    Cancel {
        id: String,
    },
    Tail {
        id: String,
        #[arg(default_value_t = 80)]
        lines: usize,
    },
    #[command(hide = true)]
    Worker {
        id: String,
        #[arg(required = true)]
        task: String,
    },
}
