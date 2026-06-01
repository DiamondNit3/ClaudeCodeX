mod agent;
mod cli;
mod config;
mod context;
mod mcp;
mod permissions;
mod providers;
mod session;
mod tools;
mod ui;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, ConfigCommand, McpCommand};
use config::AppConfig;
use providers::ProviderRegistry;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let workspace = std::env::current_dir()?;

    match cli.command.unwrap_or(Commands::Interactive) {
        Commands::Interactive => {
            let config = AppConfig::load_or_default()?;
            let registry = ProviderRegistry::from_config(&config)?;
            agent::run_interactive(config, registry, workspace).await
        }
        Commands::Exec { task } => {
            let config = AppConfig::load_or_default()?;
            let registry = ProviderRegistry::from_config(&config)?;
            agent::run_exec(config, registry, workspace, task).await
        }
        Commands::Resume { session } => {
            session::print_sessions(session.as_deref())?;
            Ok(())
        }
        Commands::Config { command } => match command {
            ConfigCommand::Init => {
                let path = AppConfig::write_default_config()?;
                println!("Wrote starter config to {}", path.display());
                Ok(())
            }
            ConfigCommand::Path => {
                println!("{}", AppConfig::config_path()?.display());
                Ok(())
            }
            ConfigCommand::Show => {
                let config = AppConfig::load_or_default()?;
                println!("{}", toml::to_string_pretty(&config)?);
                Ok(())
            }
        },
        Commands::Providers => {
            let config = AppConfig::load_or_default()?;
            for provider in ProviderRegistry::from_config(&config)?.summaries() {
                println!("{provider}");
            }
            Ok(())
        }
        Commands::Mcp { command } => match command {
            McpCommand::List => {
                let config = AppConfig::load_or_default()?;
                mcp::print_servers(&config);
                Ok(())
            }
        },
        Commands::Doctor => {
            doctor()?;
            Ok(())
        }
    }
}

fn doctor() -> Result<()> {
    println!("ClaudeCodeX doctor");
    println!("os: {}", std::env::consts::OS);
    println!("arch: {}", std::env::consts::ARCH);
    println!("cwd: {}", std::env::current_dir()?.display());
    println!("config: {}", AppConfig::config_path()?.display());

    match AppConfig::load_or_default() {
        Ok(config) => {
            println!("default provider: {}", config.default_provider);
            println!("default model: {}", config.default_model);
            println!("permission profile: {}", config.permission_profile);
            println!("configured providers: {}", config.providers.len());
            println!("configured mcp servers: {}", config.mcp.servers.len());
        }
        Err(error) => println!("config error: {error:#}"),
    }

    Ok(())
}
