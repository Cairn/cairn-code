# Cairn Code — Development Backlog

## Priority: Critical
- [x] Fix "stuck on thinking" bug — agent hangs during thinking step, suspected streaming channel lifecycle issue in non-streaming path (channels not closed properly)
- [x] Add comprehensive test suite — currently only 14 tests, need coverage for: UI spinner lifecycle, session replay, streaming drain, tool execution, error recovery, all tool implementations

## Priority: High
- [x] Add OpenCode provider support — opencode.rs
- [x] Improve error messages — actionable auth/rate-limit/network/context errors in `http_client.rs` plus clearer missing-API-key messages
- [x] Add file edit tool safety — workspace-path confinement in `file_edit`/`file_write` (`tools/workspace.rs`), `Tool::needs_permission()` gates execution, shell `timeout` enforced, in-process `file_undo` stack for edit/write

## Priority: Medium
- [x] Add configuration file support (~/.config/cairn-code/config.json) — model selection, API keys, defaults
- [x] Store API keys in the OS keyring instead of plaintext in config.json (with one-time migration of existing plaintext keys)
- [x] Improve session management — listing/save/resume/delete (`/sessions`, `/save`, `/resume`, `/delete [id]`)
- [x] Add syntax highlighting for code blocks in output — syntect (`default-fancy`) in `markdown.rs`
- [ ] Support multiple AI providers simultaneously — fallback between providers
- [x] Add cost tracking per session — token usage, estimated cost (`cost.rs`, `/cost`)
- [x] Add HTTP retry-with-backoff (429/503/529) and a stream idle-timeout watchdog (`http_client.rs`)
- [x] Thread cancellation through the streaming path and tool-execution loop (`agent.rs`, `Provider::stream_complete`), not just once per turn

## Priority: Low
- [ ] Add plugin/extension system — custom tools via Lua or WASM
- [ ] TUI theme customization — colors, styles, layout preferences
- [ ] Add completions/suggestions for commands
- [ ] Performance optimization — reduce memory allocations in hot paths
- [ ] Add benchmark tests for streaming throughput

## Larger items noted from a zero-parity audit (not started)
zero (`~/source/repos/zero`) is a much larger, mature agent CLI (~167k lines vs
cairn-code's ~6k). These are gaps confirmed against it that are each a
standalone system, not a small port:
- [x] Context/history compaction — proactive summarize-and-trim in `agent.rs` when input tokens hit 70% of model `max_ctx`
- Session fork/lineage/rewind/checkpointing
- Parallel execution of read-only tool calls
- LSP integration (diagnostics, go-to-definition)
- OS-level sandboxing for shell/tool execution
- OAuth login flows for providers (vs. raw API keys only)
- Model registry with live pricing/context-window/vision metadata
- Three-way config layering (user/project/env), currently first-match-wins
- Output/log secret redaction
- Provider catalog expansion beyond the current 5 (anthropic/openai/opencode/openrouter/ollama)

## Standing Rules
- Never force push
- Never push without reason (every commit must have purpose)
- Always run `cargo build` and `cargo test` before committing
- Always push to `origin/main`
