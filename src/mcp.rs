use crate::config::{AppConfig, McpServerConfig};

pub fn print_servers(config: &AppConfig) {
    if config.mcp.servers.is_empty() {
        println!("No MCP servers configured.");
        return;
    }

    for (name, server) in &config.mcp.servers {
        match server {
            McpServerConfig::Stdio { command, args } => {
                println!("{name}: stdio {command} {}", args.join(" "));
            }
            McpServerConfig::Http {
                url,
                bearer_token_env,
            } => {
                let auth = bearer_token_env
                    .as_ref()
                    .map(|env| format!(" auth=${env}"))
                    .unwrap_or_default();
                println!("{name}: http {url}{auth}");
            }
        }
    }
}
