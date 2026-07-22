# Cairn Code — Development Backlog

## Priority: Critical
- [x] Fix "stuck on thinking" bug — agent hangs during thinking step, suspected streaming channel lifecycle issue in non-streaming path (channels not closed properly)
- [x] Add comprehensive test suite — currently only 14 tests, need coverage for: UI spinner lifecycle, session replay, streaming drain, tool execution, error recovery, all tool implementations

## Priority: High
- [x] Add OpenCode provider support — opencode.rs
- [ ] Improve error messages — surface actionable errors when API calls fail (rate limits, auth errors, network issues)
- [ ] Add file edit tool safety — validate paths, prevent writes outside workspace, add undo support

## Priority: Medium
- [x] Add configuration file support (~/.config/cairn-code/config.json) — model selection, API keys, defaults
- [ ] Improve session management — listing/save/resume exist (`/sessions`, `/save`, `/resume`); still missing deletion from CLI
- [ ] Add syntax highlighting for code blocks in output
- [ ] Support multiple AI providers simultaneously — fallback between providers
- [x] Add cost tracking per session — token usage, estimated cost (`cost.rs`, `/cost`)

## Priority: Low
- [ ] Add plugin/extension system — custom tools via Lua or WASM
- [ ] TUI theme customization — colors, styles, layout preferences
- [ ] Add completions/suggestions for commands
- [ ] Performance optimization — reduce memory allocations in hot paths
- [ ] Add benchmark tests for streaming throughput

## Standing Rules
- Never force push
- Never push without reason (every commit must have purpose)
- Always run `cargo build` and `cargo test` before committing
- Always push to `origin/main`
