# Roadmap

## Milestone 1: MVP Runtime

- Interactive CLI
- `exec` mode
- OpenAI and Anthropic adapters
- File, search, shell, and git tools
- Approval prompts
- JSONL session logging

## Milestone 2: Usable Coding Agent

- Structured provider-native tool calling metadata and extraction
- Better patch application and diff preview summaries
- Session resume with transcript replay
- Context compaction
- Rich line-based terminal rendering
- Optional full-screen TUI behind the `tui` feature

## Milestone 3: Provider Expansion

- Gemini
- OpenRouter
- Ollama
- vLLM
- provider capability matrix

## Milestone 4: MCP and Hooks

- stdio MCP client
- streamable HTTP MCP client
- pre-tool and post-tool hooks
- command risk scoring

## Milestone 5: Advanced Agentics

- deterministic helper subagents
- review mode
- reusable workflow skills
- background tasks

## Milestone 6: Hardening and Release

- OS sandbox helpers
- protected path policy
- integration tests
- benchmark suite
- package release binary
- push to `DiamondNit3/ClaudeCodeX`

## Remaining Depth Work

- full MCP tool invocation
- provider-native streaming for OpenAI and Anthropic
- parallel model-backed subagents
- stronger OS-level process isolation
- GitHub Actions release pipeline and signed artifacts
