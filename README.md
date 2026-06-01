# ClaudeCodeX

ClaudeCodeX (`ccx`) is a terminal-only agentic coding harness inspired by Claude Code, Codex CLI, Cursor CLI, and other coding agents. It is designed as a provider-neutral runtime: the harness owns tools, permissions, project context, sessions, and terminal workflows while each model runs through its native API adapter.

## Current Status

This repository contains the first Rust implementation pass:

- interactive terminal loop
- non-interactive `exec` mode
- configurable provider/model routing
- unified effort controls for low, medium, high, and max runs
- OpenAI, Anthropic, and local HTTP provider adapters
- native Ollama adapter with local model profiles
- built-in file, search, shell, and git tools
- permission profiles for command and file access
- project instruction loading from `AGENTS.md`, `.ccx/AGENTS.md`, `CLAUDE.md`, and `.cursor/rules`
- JSONL session logging and resume listing
- MCP server config and tool visibility plumbing
- animated terminal mascot for activity feedback
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

Interactive slash commands include grouped session, model, workspace, and security controls:

```text
/help
/status
/diff
/preview
/clear
/model
/effort
/providers
/context
/permissions
/session
/compact
/mascot
/exit
```

## Configuration

`ccx config init` writes a starter config to the platform config directory. On Windows this is usually:

```text
%APPDATA%\username\ClaudeCodeX\config.toml
```

Example:

```toml
default_provider = "openai"
default_model = "gpt-5.5"
permission_profile = "ask"
effort = "medium"

[providers.openai]
kind = "openai"
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"
effort = "high"
max_output_tokens = 8192

[providers.anthropic]
kind = "anthropic"
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com/v1"
max_output_tokens = 8192

[providers.ollama]
kind = "ollama"
base_url = "http://localhost:11434"
max_output_tokens = 1024

[model_profiles."qwen3.5:0.8b"]
provider = "ollama"
supports_system = false
prefer_think_false = true
effort = "low"
tool_protocol = "simple-json"
max_tool_prompt_size = 1200
reasoning_field = true
```

Effort resolves in this order: model profile, provider, global config, then `medium`. In interactive sessions, `/effort low`, `/effort medium`, `/effort high`, or `/effort max` sets a session-only override. OpenAI receives native reasoning effort, Anthropic/local providers receive tuned output token budgets when no explicit max is configured, and Ollama receives tuned `num_predict`, `num_ctx`, and thinking flags.

## Design Principle

ClaudeCodeX does not try to bypass hidden provider policy or clone private system prompts. It keeps the harness prompt small, visible, and tool-oriented, then unlocks model capability by using provider-native APIs, transparent context, and strong local tools.

## TUI Direction

The current UI is intentionally line-based and terminal-only. A future full-screen terminal UI is staged behind the `tui` feature path so the rendering primitives can move into `ratatui` widgets without making full-screen mode mandatory.

See [docs/LOCAL_MODELS.md](docs/LOCAL_MODELS.md) for Ollama and Qwen setup notes.
