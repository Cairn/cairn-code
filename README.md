# Cairn Code

[![Rust](https://img.shields.io/badge/rust-1.96-orange?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-private-lightgrey)](#)
[![Providers](https://img.shields.io/badge/LLM-Anthropic%20%7C%20OpenAI%20%7C%20OpenRouter%20%7C%20OpenGateway%20%7C%20Ollama-blue)](#)
[![TUI](https://img.shields.io/badge/TUI-ratatui-yellow)](https://ratatui.rs/)
[![Tools](https://img.shields.io/badge/tools-13-brightgreen)](#)

A Rust-based CLI LLM coding agent, inspired by Claude Code. Built by Cairn.

## Features

- **Multi-provider LLM support** — Anthropic (Claude), OpenAI (GPT), OpenRouter, OpenGateway (GitLawb smart router), Ollama
- **Agentic tool loop** — The LLM autonomously reads files, writes code, runs commands, and searches your codebase until the task is done
- **13 built-in tools** — FileRead, FileWrite, FileEdit, FileUndo, Shell, Go, Git, Glob, Grep, Memory, WebSearch, WebFetch, TodoWrite
- **Real-time streaming** — Token-by-token output with live tool display and thinking blocks
- **Ratatui TUI** — Terminal UI with input history, spinner, provider/model pickers, and syntect-highlighted fenced code blocks
- **Cost tracking** — Per-session and per-tool-call token usage with cache-aware pricing
- **Permission system** — Per-tool auto_allow/ask/deny configuration
- **Print mode** — Non-interactive execution for scripting and pipelines
- **Minimal deps** — `ratatui`, `keyring`, and `syntect` for code fences; JSON is a hand-written recursive descent parser; LLM HTTP and web tools shell out to `curl`

## Quick Start

### Prerequisites

- Rust 1.96+
- [curl](https://curl.se/) installed and on PATH (used by the `web_fetch` and `web_search` tools)

### Build

```bash
cargo build --release
```

### Configure

Set your API key:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
# or
export OPENAI_API_KEY="sk-..."
# or
export OPENROUTER_API_KEY="sk-or-..."
# or GitLawb OpenGateway (smart-routes by model id)
export GITLAWB_OPENGATEWAY_API_KEY="ogw_live_..."
```

Anthropic is the default provider. OpenGateway is an OpenAI-compatible gateway (`https://opengateway.gitlawb.com/v1`) that routes by model id. Ollama talks to a local server and needs no cloud API key.

Optionally create a config file:

```json
// ~/.config/cairn-code/config.json
{
  "default_provider": "anthropic",
  "default_model": "claude-sonnet-4-20250514",
  "max_turns": 50,
  "max_tokens": 8192,
  "permissions": {
    "auto_allow": ["file_read", "glob", "grep"],
    "ask": ["file_write", "shell", "file_edit"]
  }
}
```

### Run

```bash
# Interactive REPL
cargo run

# One-shot prompt
cargo run "explain this codebase"

# Print mode (non-interactive)
cargo run -p "list all files in this project"
```

## Architecture

```
src/
  main.rs                Entry point, argument parsing, agent thread launch
  agent.rs               Core agentic loop (LLM call -> tool use -> repeat)
  config.rs              Configuration loading and merging (JSON + env vars)
  cost.rs                Model pricing tables and cost estimation
  http_client.rs         HTTP client via ureq (blocking + streaming)
  json.rs                Hand-written recursive descent JSON parser
  markdown.rs            Markdown rendering + syntect code-block highlighting
  session.rs             Session persistence (save/load/list)
  tui.rs                 Ratatui terminal UI
  llm/
    provider.rs          Shared types (Message, Content, ToolDefinition, Provider trait)
    anthropic.rs         Anthropic Messages API client (SSE streaming)
    openai.rs            OpenAI Chat Completions client (streaming)
    openrouter.rs        OpenRouter client (OpenAI-compatible, streaming)
    opengateway.rs       GitLawb OpenGateway (OpenAI-compatible smart router)
    ollama.rs            Local Ollama client
  tools/
    registry.rs          Tool trait and registry
    file_read.rs         Read files with line pagination
    file_write.rs        Create/overwrite files
    file_edit.rs         Find-and-replace editing
    file_history.rs      In-process undo stack for edit/write
    file_undo.rs         Undo last file_edit/file_write
    shell.rs             Shell command execution with timeout
    go_tool.rs           Go command execution (no shell injection)
    git_tool.rs          Git command execution (no shell injection)
    glob_tool.rs         File pattern matching (glob)
    grep_tool.rs         Regex search across codebase
    memory.rs            Cross-session memory storage/retrieval
    web_search.rs        DuckDuckGo web search (via curl)
    web_fetch.rs         HTTP page fetcher with HTML-to-text (via curl)
    todo.rs              Task tracking
```

### Agent Loop

```
User Prompt -> Build System Prompt (CAIRN.md + Todos + Tools)
           -> Call LLM with tools (streaming)
           -> Process Response:
             +- Text -> Display to user
             +- Thinking -> Display thinking blocks
             +- Tool Use -> Check permissions -> Execute -> Append result -> Loop
           -> end_turn -> Wait for next input
```

## Tools

| Tool | Description | Needs Permission |
|------|-------------|:---:|
| **file_read** | Read files with line numbers, offset/limit pagination | No |
| **file_write** | Create or overwrite files | Yes |
| **file_edit** | Find-and-replace editing | Yes |
| **file_undo** | Undo last file_edit/file_write in this process | Yes |
| **shell** | Execute shell commands with timeout | Yes |
| **go** | Execute Go commands (avoids shell injection) | Yes |
| **git** | Execute Git commands (avoids shell injection) | Yes |
| **glob** | File pattern matching | No |
| **grep** | Regex search across the codebase | No |
| **memory** | Store and retrieve cross-session information | No |
| **web_search** | Search the web via DuckDuckGo | No |
| **web_fetch** | Fetch and extract web page content | Yes |
| **todo_write** | Manage a task/todo list | No |

## REPL Commands

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/clear` | Clear conversation history |
| `/compact` | Summarize older history into a shorter context now |
| `/model` | Show or change the current model |
| `/cost` | Show token usage for the session |
| `/provider` | Show or change the current provider |
| *(after LLM error)* | Prompt: switch model (`m`), switch provider (`p`), or dismiss (`d`/Esc) |
| `/save` | Save the current session |
| `/sessions` | List saved sessions |
| `/resume` | Resume a saved session (picker) |
| `/delete` | Delete a saved session (picker, or `/delete <id-prefix>`) |
| `/quit`, `/exit`, `/q` | Exit Cairn Code |

## Configuration Reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_provider` | string | `"anthropic"` | LLM provider (`anthropic`, `openai`, `openrouter`, `opengateway`, `ollama`) |
| `default_model` | string | `"claude-sonnet-4-20250514"` | Default model identifier |
| `max_turns` | int | `100` | Maximum agent loop iterations |
| `max_tokens` | int | `8192` | Max tokens per LLM response |
| `system_prompt_file` | string | `"CAIRN.md"` | File to load as system prompt |
| `permissions.auto_allow` | []string | `[]` | Tools to auto-approve |
| `permissions.ask` | []string | `[]` | Tools that require confirmation |
| `permissions.deny` | []string | `[]` | Tools to block |

## License

Proprietary -- (c) 2026 Cairn. All rights reserved. No license is granted. Do not reproduce, distribute, or use outside the Cairn organization without written permission.
