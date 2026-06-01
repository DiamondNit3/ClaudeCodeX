# ClaudeCodeX Architecture

## Runtime Loop

1. Load global config and project context.
2. Start or resume a JSONL session.
3. Build a provider-native request from:
   - transparent harness prompt
   - loaded project instructions
   - user task
   - available tool schemas
   - resolved effort level
   - prior session events
4. Stream model output when the provider supports it, otherwise render activity while waiting.
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
- native Ollama `/api/chat`

Providers expose `ProviderCapabilities` for streaming and native tools. Ollama currently streams newline-delimited chat chunks into the terminal. OpenAI and Anthropic receive provider-native tool schemas and translate native tool calls back into the harness tool-call loop.

## Effort Resolution

ClaudeCodeX exposes one effort control with four values: `low`, `medium`, `high`, and `max`. The runtime resolves effort in this order:

1. model profile
2. provider config
3. global config
4. `medium`

Interactive `/effort` changes apply only to the current session. Provider adapters translate the resolved value into native request fields where possible: OpenAI receives reasoning effort, Anthropic and local OpenAI-compatible providers receive output token defaults, and Ollama receives `num_predict`, `num_ctx`, and thinking mode hints.

## Tool Boundary

The tool layer is owned by the harness, not the model provider. Built-in tools currently include:

- `read_file`
- `write_file`
- `edit_file`
- `apply_patch`
- `glob`
- `grep`
- `shell`
- `git_status`
- `git_diff`

Tool input and output are plain JSON values. Built-in tools can be exposed as provider-native schemas through `ToolSpec`, while XML/JSON parsing remains the fallback path for local or unsupported models.

The parser accepts strict XML tool calls, fenced JSON, bare JSON, local-model action JSON, provider-native tool calls converted into XML, and inferred file outputs for common cases such as raw HTML.

## Patch Engine

`edit_file` supports exact search/replace and a guarded unified-diff subset. `apply_patch` applies one-file unified hunks only when the old context is found exactly, then reports a diff summary. This keeps first-pass patching deterministic and easy to reject on conflict.

## Permissions

Permission profiles are enforced before tool execution:

- `read-only`: no writes or shell commands
- `ask`: prompt before writes and commands
- `workspace-write`: allow file edits in the workspace, ask for shell
- `full-access`: allow workspace file edits and shell
- `danger-full-access`: no harness-level checks

This pass implements harness-level checks. OS-level sandboxing is a future hardening layer.

Additional safety layers now run before tool execution:

- protected path policy for `.git`, credentials, SSH keys, environment files, and system paths
- shell command risk scoring for read-only, write, network, package install, destructive, credential, and privilege commands
- configurable pre-tool and post-tool hooks that receive structured JSON through `CCX_HOOK_PAYLOAD`

## Context Loading

The context loader scans the workspace for durable instruction files:

- `AGENTS.md`
- `.ccx/AGENTS.md`
- `CLAUDE.md`
- `.cursor/rules`

The effective context can be shown with `/context`.

## Compaction

`/compact` summarizes older transcript turns into a `compact_summary` session event and keeps recent messages live. Resumed sessions rehydrate compact summaries as system context so long runs preserve goals, tool-result counts, and touched files.

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

## Agent Power Features

- `ccx review` and `/review` inspect diffs for common risk patterns.
- `ccx skills` and `/skills` discover Markdown workflow skills from `.ccx/skills` and the user data directory.
- `ccx subagent` and `/subagent` provide deterministic helper roles for search, review, test-debug, and planning.
- `ccx task` tracks background task metadata and logs.
- `ccx bench` emits benchmark smoke checks as JSON.
- `ccx release-check` prints the release verification checklist.
