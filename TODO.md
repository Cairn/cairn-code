# Cairn Code — Development Backlog

## Priority: Critical
- [x] Fix "stuck on thinking" bug — agent hangs during thinking step, suspected streaming channel lifecycle issue in non-streaming path (channels not closed properly)
- [x] Add comprehensive test suite — currently only 14 tests, need coverage for: UI spinner lifecycle, session replay, streaming drain, tool execution, error recovery, all tool implementations

## Priority: High
- [ ] Fix middleware.test.ts audit log failures — 3 tests fail with "Cannot use a closed database" in middleware.test.ts (note: this is Synapse CRM, reference for pattern)
- [x] Add OpenCode provider support — opencode.go exists but needs integration testing
- [ ] Improve error messages — surface actionable errors when API calls fail (rate limits, auth errors, network issues)
- [ ] Add file edit tool safety — validate paths, prevent writes outside workspace, add undo support

## Priority: Medium
- [ ] Add configuration file support (~/.cairn/config.toml) — model selection, API keys, defaults
- [ ] Improve session management — session listing, switching, deletion from CLI
- [ ] Add syntax highlighting for code blocks in output
- [ ] Support multiple AI providers simultaneously — fallback between providers
- [ ] Add cost tracking per session — token usage, estimated cost

## Priority: Low
- [ ] Add plugin/extension system — custom tools via Lua or WASM
- [ ] TUI theme customization — colors, styles, layout preferences
- [ ] Add completions/suggestions for commands
- [ ] Performance optimization — reduce memory allocations in hot paths
- [ ] Add benchmark tests for streaming throughput

## Standing Rules
- Never force push
- Never push without reason (every commit must have purpose)
- Always use `git -C /home/z/my-project/cairn-code`
- Always commit as `SuperDuperZed`
- Always run `go build ./...` and `go test ./...` before committing
- Always push to `origin/main`
