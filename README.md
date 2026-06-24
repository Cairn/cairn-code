# Cairn Code

A Claude Code-style terminal coding agent built in Go. Streams LLM responses in real-time with a rich TUI, cost tracking, and a full tool suite for software engineering tasks.

## Features

- Real-time streaming output with live tool display
- Viewport scrolling, content caching, and keyboard shortcuts
- Multi-provider LLM support (Anthropic, OpenAI, Ollama, OpenCode)
- Cost tracking per session and per tool call
- Built-in tools: file read/write/edit, bash, go, git, grep, glob, web search/fetch, todo management
- Diff-aware file editing

## Build

```bash
make build
./cairn-code
```

## Tools

| Tool | Description |
|------|-------------|
| `file_read` | Read file contents with optional offset/limit |
| `file_write` | Create or overwrite files |
| `file_edit` | Make targeted edits to existing files |
| `bash` | Execute shell commands |
| `go` | Execute go commands |
| `git` | Execute git commands |
| `grep` | Search file contents with regex |
| `glob` | Find files by pattern |
| `web_search` | Search the web |
| `web_fetch` | Fetch and extract web page content |
| `todo` | Manage task lists |

## License

MIT
