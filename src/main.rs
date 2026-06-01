mod agent;
mod bench;
mod cli;
mod config;
mod context;
mod hooks;
mod mcp;
mod parser;
mod patch;
mod permissions;
mod preview;
mod providers;
mod release;
mod review;
mod safety;
mod session;
mod skills;
mod subagents;
mod tasks;
mod tools;
mod ui;

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Commands, ConfigCommand, McpCommand, TaskCommand};
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
            if let Some(session) = session {
                let config = AppConfig::load_or_default()?;
                let registry = ProviderRegistry::from_config(&config)?;
                agent::run_resume(config, registry, workspace, session).await
            } else {
                session::print_sessions(None)?;
                Ok(())
            }
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
        Commands::Review { paths } => review::run_review(&workspace, &paths),
        Commands::Skills => skills::print_skills(&workspace),
        Commands::Task { command } => match command {
            TaskCommand::Spawn { task } => tasks::spawn_task(&workspace, &task),
            TaskCommand::List => tasks::list_tasks(),
            TaskCommand::Show { id } => tasks::show_task(&id),
            TaskCommand::Cancel { id } => tasks::cancel_task(&id),
            TaskCommand::Tail { id, lines } => tasks::tail_task(&id, lines),
            TaskCommand::Worker { id, task } => tasks::run_worker(&id, &task),
        },
        Commands::Subagent { kind, task } => {
            let config = AppConfig::load_or_default()?;
            let registry = ProviderRegistry::from_config(&config)?;
            let report =
                subagents::run_subagent(&config, &registry, workspace, &kind, &task, false).await?;
            println!(
                "subagent: {}  tool calls: {}\n{}",
                report.kind, report.tool_calls, report.report
            );
            Ok(())
        }
        Commands::Bench => bench::run_bench(&workspace),
        Commands::ReleaseCheck => release::print_release_check(),
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
            println!("global effort: {}", config.effort);
            println!(
                "resolved default effort: {}",
                config.resolve_effort(&config.default_provider, &config.default_model)
            );
            println!("permission profile: {}", config.permission_profile);
            println!("configured providers: {}", config.providers.len());
            println!("configured model profiles: {}", config.model_profiles.len());
            println!("configured mcp servers: {}", config.mcp.servers.len());
            println!(
                "configured hooks: {} pre, {} post",
                config.hooks.pre_tool.len(),
                config.hooks.post_tool.len()
            );
        }
        Err(error) => println!("config error: {error:#}"),
    }

    Ok(())
}
