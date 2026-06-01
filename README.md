# ClaudeCodeX

ClaudeCodeX (`ccx`) is a terminal-only agentic coding harness inspired by Claude Code, Codex CLI, Cursor CLI, and other coding agents. It is designed as a provider-neutral runtime: the harness owns tools, permissions, project context, sessions, and terminal workflows while each model runs through its native API adapter.

## Current Status

This repository contains the first Rust implementation pass:

- interactive terminal loop
- non-interactive `exec` mode
- configurable provider/model routing
- OpenAI, Anthropic, and local HTTP provider adapters
- built-in file, search, shell, and git tools
- permission profiles for command and file access
- project instruction loading from `AGENTS.md`, `.ccx/AGENTS.md`, `CLAUDE.md`, and `.cursor/rules`
- JSONL session logging and resume listing
- MCP server config and tool visibility plumbing
- small, transparent base prompt

Real OS-level sandboxing, full MCP invocation, subagents, and rich ratatui rendering are planned next.

## Commands

```powershell
ccx
ccx exec "fix the failing tests"
ccx resume
ccx config init
ccx providers
ccx mcp list
ccx doctor
```

## Configuration

`ccx config init` writes a starter config to the platform config directory. On Windows this is usually:

```text
%APPDATA%\DiamondNit3\ClaudeCodeX\config.toml
```

Example:

```toml
default_provider = "openai"
default_model = "gpt-5.5"
permission_profile = "ask"

[providers.openai]
kind = "openai"
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"
reasoning_effort = "high"
max_output_tokens = 8192

[providers.anthropic]
kind = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com/v1"
max_output_tokens = 8192
```

## Design Principle

ClaudeCodeX does not try to bypass hidden provider policy or clone private system prompts. It keeps the harness prompt small, visible, and tool-oriented, then unlocks model capability by using provider-native APIs, transparent context, and strong local tools.
