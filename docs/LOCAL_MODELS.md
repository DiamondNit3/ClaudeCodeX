# Local Models

ClaudeCodeX supports local models through Ollama. Local models can be useful for private, offline, or low-cost workflows, but small models often need simpler prompts and more forgiving tool parsing than frontier models.

## Ollama Provider

Use the native Ollama provider for best compatibility:

```toml
default_provider = "ollama"
default_model = "qwen3.5:0.8b"
permission_profile = "workspace-write"
effort = "low"

[providers.ollama]
kind = "ollama"
base_url = "http://localhost:11434"
max_output_tokens = 1024
```

The native provider uses `POST /api/chat`. It does not rely on Ollama's OpenAI-compatible `/v1/chat/completions` endpoint.

## Model Profiles

Profiles tune the harness for specific local models:

```toml
[model_profiles."qwen3.5:0.8b"]
provider = "ollama"
supports_system = false
prefer_think_false = true
effort = "low"
tool_protocol = "simple-json"
max_tool_prompt_size = 1200
reasoning_field = true
context_budget = 4096
notes = "Small local model profile optimized for short prompts."
```

Profile behavior:

- `supports_system = false` flattens system instructions into user-visible context.
- `prefer_think_false = true` asks Ollama to suppress thinking when supported.
- `effort = "low"` keeps small models on shorter outputs and smaller context defaults.
- `tool_protocol = "simple-json"` uses shorter local-model tool instructions.
- `reasoning_field = true` lets the adapter recover text from Ollama reasoning fields when content is empty.

## Effort

Effort can be set globally, per provider, per model profile, or interactively with `/effort low|medium|high|max`. Native Ollama requests map effort to `num_predict`, `num_ctx`, and thinking mode. For small models such as `qwen3.5:0.8b`, keep `prefer_think_false = true` and `effort = "low"` unless a task needs longer reasoning or larger output.

## Tool Fallbacks

Local models may return raw file content instead of a formal tool call. ClaudeCodeX can recover common cases:

- strict `<tool_call>{...}</tool_call>`
- fenced JSON
- bare JSON
- simple action JSON, such as `{"action":"write_file","path":"index.html","content":"..."}`
- partial XML with valid JSON inside
- raw HTML when the user asked to create an HTML file

For example, this can create `index.html` even if the model returns a fenced HTML block:

```powershell
ccx exec "Create a simple self-contained index.html page with a button interaction."
```

## Preview

Inside interactive mode:

```text
/preview index.html
```

ClaudeCodeX starts a local static server and prints a URL like:

```text
preview  http://127.0.0.1:4317/index.html
```

The server stays alive for the current session and stops when the session ends.

## Limitations

Small models may still ignore instructions, over-explain, or produce malformed JSON. Model profiles, fallback parsing, and inferred file writes improve reliability, but larger local coding models will behave better.
