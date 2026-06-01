# ClaudeCodeX Architecture

## Runtime Loop

1. Load global config and project context.
2. Start or resume a JSONL session.
3. Build a provider-native request from:
   - transparent harness prompt
   - loaded project instructions
   - user task
   - available tool schemas
   - prior session events
4. Stream model output.
5. Execute requested tools after permission checks.
6. Feed tool results back to the model.
7. Persist every message, tool call, decision, and result.
8. Stop when the model reports completion, the user exits, or a permission/blocking condition stops progress.

## Provider Boundary

The provider layer exposes a common `ModelProvider` trait while preserving provider-specific request code internally. Each adapter declares capabilities so the runtime can avoid flattening every model into the same behavior.

Supported first-pass providers:

- OpenAI Responses API
- Anthropic Messages API
- local OpenAI-compatible HTTP endpoints

## Tool Boundary

The tool layer is owned by the harness, not the model provider. Built-in tools currently include:

- `read_file`
- `write_file`
- `edit_file`
- `glob`
- `grep`
- `shell`
- `git_status`
- `git_diff`

Tool input and output are plain JSON values so they can be bridged to provider-native tool calling and MCP later.

## Permissions

Permission profiles are enforced before tool execution:

- `read-only`: no writes or shell commands
- `ask`: prompt before writes and commands
- `workspace-write`: allow file edits in the workspace, ask for shell
- `full-access`: allow workspace file edits and shell
- `danger-full-access`: no harness-level checks

This pass implements harness-level checks. OS-level sandboxing is a future hardening layer.

## Context Loading

The context loader scans the workspace for durable instruction files:

- `AGENTS.md`
- `.ccx/AGENTS.md`
- `CLAUDE.md`
- `.cursor/rules`

The effective context can be shown with `/context`.

## Session Format

Sessions are JSONL files stored in the user data directory. Events include:

- user input
- assistant output
- tool calls
- tool results
- approvals
- compaction notes
- errors

JSONL keeps sessions append-only, streamable, and easy to inspect.
