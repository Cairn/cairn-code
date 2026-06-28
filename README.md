<p align="center">
  <img src="https://img.shields.io/badge/Go-1.24-00ADD8?style=for-the-badge&logo=go&logoColor=white" alt="Go"><br>
  <img src="https://img.shields.io/badge/CLI-agentic%20loop-6e40c9?style=for-the-badge" alt="Agent Loop"><br>
  <img src="https://img.shields.io/badge/LLM-Anthropic%20%7C%20OpenAI%20%7C%20Ollama%20%7C%20OpenCode-f472b6?style=for-the-badge" alt="Multi-Provider"><br>
  <img src="https://img.shields.io/badge/TUI-bubbletea-FF6F00?style=for-the-badge" alt="Bubbletea"><br>
  <img src="https://img.shields.io/badge/Tools-11-22c55e?style=for-the-badge" alt="11 Tools"><br>
  <img src="https://img.shields.io/badge/Private-Cairn-1a1b26?style=for-the-badge" alt="Private">
</p>

<h1 align="center">Cairn Code</h1>

<p align="center">
  <strong>A Go-based CLI LLM coding agent, inspired by Claude Code.</strong><br>
  <i>Built by Cairn</i>
</p>

## Screenshots

<p align="center">
  <img src="screenshots/repl-startup.png" width="600" alt="Cairn Code REPL Startup"><br>
 <i>REPL startup with command reference</i>
</p>

<p align="center">
  <img src="screenshots/agent-action.png" width="600" alt="Cairn Code Agent in Action"><br>
  <i>Agent autonomously using Grep to search the codebase</i>
</p>

<p align="center">
  <img src="screenshots/print-mode.png" width="600" alt="Cairn Code Print Mode"><br>
  <i>Non-interactive print mode for scripting and pipelines</i>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/platforms-linux%20%7C%20macos%20%7C%20windows-informational?style=flat-square" alt="Platforms">
  <img src="https://img.shields.io/badge/PRs-welcome-FF69B4?style=flat-square" alt="PRs Welcome">
</p>

## Features

- **Multi-provider LLM support** — Anthropic (Claude), OpenAI (GPT), Ollama (local models), OpenCode (free API)
- **Agentic tool loop** — The LLM autonomously reads files, writes code, runs commands, and searches your codebase until the task is done
- **11 built-in tools** — FileRead, FileWrite, FileEdit, Bash, Go, Git, Glob, Grep, WebSearch, WebFetch, TodoWrite
- **Real-time streaming** — Token-by-token output with live tool display and thinking blocks
- **Rich terminal REPL** — Bubbletea-powered TUI with markdown rendering, viewport scrolling, input history, and spinner animations
- **Cost tracking** — Per-session and per-tool-call token usage with cache-aware pricing
- **Permission system** — Per-tool allow/ask/deny configuration with pattern matching
- **Session management** — Save, resume, and list conversation sessions
- **Print mode** — Non-interactive execution for scripting and pipelines
- **Configurable** — Global (`~/.config/cairn-code/config.json`) and project-local (`.cairn/config.json`) settings

## Quick Start

### Install

```bash
go install ./cmd/cairn-code@latest
```

### Configure

Set your API key:

```bash
export ANTHROPIC_API_KEY="sk-ant-..."
# or
export OPENAI_API_KEY="sk-..."
```

Optionally create a config file:

```json
// ~/.config/cairn-code/config.json
{
  "default_provider": "anthropic",
  "default_model": "claude-sonnet-4-20250514",
  "anthropic": {
    "api_key": "sk-ant-..."
  },
  "max_turns": 50,
  "permissions": {
    "auto_allow": ["file_read", "glob", "grep"],
    "ask": ["file_write", "bash", "file_edit"]
  }
}
```

### Run

```bash
# Interactive REPL
cairn-code

# One-shot prompt
cairn-code "explain this codebase"

# Print mode (non-interactive)
cairn-code -p "list all go files in this project"

# Override provider/model
cairn-code --provider ollama --model llama3 "write a hello world"
```

## Architecture

```
cmd/cairn-code/main.go     Entry point, flag parsing, REPL launch
internal/
  agent/agent.go           Core agentic loop (LLM call → tool use → repeat)
  config/config.go         Configuration loading and merging
  cost/cost.go             Model pricing tables and cost estimation
  llm/
    provider.go            Shared types (Message, ContentBlock, ToolDefinition)
    anthropic.go           Anthropic Messages API client (streaming + thinking)
    openai.go             OpenAI Chat Completions client (streaming)
    ollama.go             Ollama local model client
    opencode.go           OpenCode free API client (streaming)
    factory.go            Provider factory
  session/session.go       Session persistence (save/load/list)
  tools/
    registry.go           Tool registry and discovery
    file_read.go          Read files with line pagination
    file_write.go         Create/overwrite files
    file_edit.go          Find-and-replace editing with diffs
    bash.go               Shell command execution with timeouts
    go.go                 Go command execution (no shell injection)
    git.go                Git command execution (no shell injection)
    glob.go               File pattern matching (doublestar)
    grep.go               Regex search across codebase
    web_search.go         DuckDuckGo web search
    web_fetch.go          HTTP page fetcher with HTML-to-text
    todo.go               Task tracking
  ui/repl.go              Bubbletea REPL with markdown rendering
pkg/
  diff/diff.go            LCS-based unified diff
```

### Agent Loop

```
User Prompt → Build System Prompt (CAIRN.md + Todos + Tools)
           → Call LLM with tools (streaming if supported)
           → Process Response:
             ├── Text → Display to user
             ├── Thinking → Display thinking blocks
             └── Tool Use → Check permissions → Execute → Append result → Loop
           → end_turn → Wait for next input
```

## Tools

| Tool | Description | Needs Permission |
|------|-------------|:---:|
| **file_read** | Read files with line numbers, offset/limit pagination | No |
| **file_write** | Create or overwrite files | Yes |
| **file_edit** | Find-and-replace editing with diff output | Yes |
| **bash** | Execute shell commands with timeout | Yes |
| **go** | Execute Go commands (avoids shell injection) | Yes |
| **git** | Execute Git commands (avoids shell injection) | Yes |
| **glob** | File pattern matching with doublestar | No |
| **grep** | Regex search across the codebase | No |
| **web_search** | Search the web via DuckDuckGo | No |
| **web_fetch** | Fetch and extract web page content | Yes |
| **todo_write** | Manage a task/todo list | No |

## REPL Commands

| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/clear` | Clear conversation history |
| `/model` | Show or change the current model |
| `/compact` | Summarize and compact the conversation |
| `/cost` | Show token usage for the session |
| `/provider` | Show current provider |
| `/save` | Save current session to disk |
| `/resume` | Resume a saved session |
| `/sessions` | List all saved sessions |
| `/tools` | List available tools |
| `/undo` | Remove the last exchange |
| `/quit` | Exit Cairn Code |

## Configuration Reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_provider` | string | `"anthropic"` | LLM provider (`anthropic`, `openai`, `ollama`, `opencode`) |
| `default_model` | string | `"claude-sonnet-4-20250514"` | Default model identifier |
| `max_turns` | int | `100` | Maximum agent loop iterations |
| `max_tokens` | int | `8192` | Max tokens per LLM response |
| `system_prompt_file` | string | `"CAIRN.md"` | File to load as system prompt |
| `permissions.auto_allow` | []string | `[]` | Tools to auto-approve |
| `permissions.ask` | []string | `[]` | Tools that require confirmation |
| `permissions.deny` | []string | `[]` | Tools to block |

## License

Proprietary — © 2026 Cairn. All rights reserved. No license is granted. Do not reproduce, distribute, or use outside the Cairn organization without written permission.
