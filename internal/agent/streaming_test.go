package agent

import (
        "context"
        "errors"
        "strings"
        "testing"
        "time"

        "github.com/cairn/cairn-code/internal/config"
        "github.com/cairn/cairn-code/internal/llm"
        "github.com/cairn/cairn-code/internal/tools"
)

// TestStreamingProviderIsUsed verifies that when a provider implements
// StreamingProvider, the streaming path is taken (OnStreamChunk fires).
func TestStreamingProviderIsUsed(t *testing.T) {
        streamCalled := false
        provider := &mockStreamingProvider{
                mockProvider: &mockProvider{name: "mock-stream"},
                streamFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string, cb llm.StreamingCallback) (*llm.Response, error) {
                        streamCalled = true
                        cb("chunk1", "text", false)
                        cb("chunk2", "text", false)
                        cb("", "text", true)
                        return &llm.Response{
                                Content:    []llm.ContentBlock{{Type: "text", Text: "chunk1chunk2"}},
                                StopReason: "end_turn",
                        }, nil
                },
        }

        cfg := &config.Config{DefaultModel: "mock-model", MaxTurns: 10}
        a := NewAgent(provider, tools.NewRegistry(), cfg, &tools.TodoStore{})

        var chunkCalls []string
        a.SetCallbacks(Callbacks{
                OnStreamChunk: func(chunk string) {
                        chunkCalls = append(chunkCalls, chunk)
                },
        })

        err := a.Run(context.Background(), "test")
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }

        if !streamCalled {
                t.Error("expected StreamMessage to be called (provider implements StreamingProvider)")
        }
        if len(chunkCalls) != 2 {
                t.Errorf("expected 2 OnStreamChunk calls, got %d", len(chunkCalls))
        }
}

// TestNonStreamingProviderFallback verifies that a provider without
// StreamingProvider still works via SendMessage.
func TestNonStreamingProviderFallback(t *testing.T) {
        provider := &mockProvider{
                name: "mock-nonstream",
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        return &llm.Response{
                                Content:    []llm.ContentBlock{{Type: "text", Text: "plain response"}},
                                StopReason: "end_turn",
                        }, nil
                },
        }

        cfg := &config.Config{DefaultModel: "mock-model", MaxTurns: 10}
        a := NewAgent(provider, tools.NewRegistry(), cfg, &tools.TodoStore{})

        streamCalled := false
        a.SetCallbacks(Callbacks{
                OnStreamChunk: func(chunk string) {
                        streamCalled = true
                },
                OnText: func(text string) {
                        // Should fire for non-streaming providers
                },
        })

        err := a.Run(context.Background(), "test")
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }

        // Non-streaming provider should NOT fire OnStreamChunk
        if streamCalled {
                t.Error("OnStreamChunk should not be called for non-streaming providers")
        }
}

// TestStreamingToolUseAccumulation verifies tool_use blocks are correctly
// accumulated during streaming and then executed.
func TestStreamingToolUseAccumulation(t *testing.T) {
        provider := &mockStreamingProvider{
                mockProvider: &mockProvider{name: "mock-stream"},
                streamFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string, cb llm.StreamingCallback) (*llm.Response, error) {
                        // Simulate a response with text + tool_use
                        cb("Let me run that.", "text", false)
                        cb("", "text", true)
                        return &llm.Response{
                                Content: []llm.ContentBlock{
                                        {Type: "text", Text: "Let me run that."},
                                        {Type: "tool_use", ID: "call_1", Name: "bash", Input: map[string]any{"command": "echo hello"}},
                                },
                                StopReason: "tool_use",
                        }, nil
                },
        }

        a := newTestAgentWithTools(provider, []tools.Tool{
                &mockTool{name: "bash", description: "run commands"},
        })

        var toolName string
        a.SetCallbacks(Callbacks{
                OnStreamChunk: func(chunk string) {},
                OnText: func(text string) {},
                OnToolUse: func(name string, input any) {
                        toolName = name
                },
                OnToolResult: func(name string, output string, duration time.Duration) {},
        })

        err := a.Run(context.Background(), "run echo hello")
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }

        if toolName != "bash" {
                t.Errorf("expected tool name 'bash', got '%s'", toolName)
        }
}

// TestAgentPanicRecovery verifies that a panicking provider doesn't leave
// channels unclosed (simulating the goroutine's defer/recover behavior).
func TestAgentPanicRecovery(t *testing.T) {
        // This test verifies the pattern used in the goroutine:
        // defer func() { recover(); close(chunkCh) }()
        // If the agent panics, chunkCh must still be closed.

        provider := &mockProvider{
                name: "mock-panic",
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        panic("agent exploded")
                },
        }

        a := newTestAgent(provider)

        // Simulate the goroutine pattern from repl.go
        chunkCh := make(chan string, 256)
        type agentResult struct {
                err error
        }
        resultCh := make(chan agentResult, 1)

        go func() {
                defer func() {
                        if r := recover(); r != nil {
                                select {
                                case resultCh <- agentResult{
                                        err: errors.New(r.(string)),
                                }:
                                default:
                                }
                        }
                        close(chunkCh)
                }()

                a.Run(context.Background(), "test")
                resultCh <- agentResult{}
        }()

        // Verify chunkCh is closed (doesn't block forever)
        for {
                _, ok := <-chunkCh
                if !ok {
                        break // channel closed — success
                }
        }

        // Verify resultCh has the error
        select {
        case result := <-resultCh:
                if result.err == nil {
                        t.Error("expected error result from panic, got nil")
                } else if !strings.Contains(result.err.Error(), "agent exploded") {
                        t.Errorf("expected panic error, got: %v", result.err)
                }
        default:
                t.Error("expected result on resultCh, got nothing")
        }
}

// TestAgentMultipleTurnStreaming verifies streaming works across multiple
// agent turns (tool use → tool result → final text).
func TestAgentMultipleTurnStreaming(t *testing.T) {
        callCount := 0
        provider := &mockStreamingProvider{
                mockProvider: &mockProvider{name: "mock-stream"},
                streamFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string, cb llm.StreamingCallback) (*llm.Response, error) {
                        callCount++
                        if callCount == 1 {
                                cb("Checking", "text", false)
                                cb(" files", "text", false)
                                cb("", "text", true)
                                return &llm.Response{
                                        Content: []llm.ContentBlock{
                                                {Type: "text", Text: "Checking files"},
                                                {Type: "tool_use", ID: "call_1", Name: "bash", Input: map[string]any{"command": "ls"}},
                                        },
                                        StopReason: "tool_use",
                                }, nil
                        }
                        // Second turn
                        cb("Done", "text", false)
                        cb("", "text", true)
                        return &llm.Response{
                                Content:    []llm.ContentBlock{{Type: "text", Text: "Done"}},
                                StopReason: "end_turn",
                                Usage:      llm.Usage{InputTokens: 20, OutputTokens: 10},
                        }, nil
                },
        }

        a := newTestAgentWithTools(provider, []tools.Tool{
                &mockTool{name: "bash", description: "run commands"},
        })

        var allChunks []string
        var turnCount int
        a.SetCallbacks(Callbacks{
                OnStreamChunk: func(chunk string) {
                        allChunks = append(allChunks, chunk)
                },
                OnText: func(text string) {},
                OnTurnEnd: func(turn int, usage llm.Usage) {
                        turnCount = turn
                },
        })

        err := a.Run(context.Background(), "list files")
        if err != nil {
                t.Fatalf("unexpected error: %v", err)
        }

        if callCount != 2 {
                t.Errorf("expected 2 provider calls, got %d", callCount)
        }
        if turnCount != 2 {
                t.Errorf("expected 2 turns, got %d", turnCount)
        }

        // Check streaming chunks across both turns
        fullText := strings.Join(allChunks, "")
        if fullText != "Checking filesDone" {
                t.Errorf("expected 'Checking filesDone', got '%s'", fullText)
        }
}

// TestAgentErrorCallbackNonStreaming verifies OnError fires for non-streaming
// provider errors.
func TestAgentErrorCallbackNonStreaming(t *testing.T) {
        provider := &mockProvider{
                sendFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string) (*llm.Response, error) {
                        return nil, errors.New("API server unavailable")
                },
        }

        a := newTestAgent(provider)

        var errorReceived bool
        a.SetCallbacks(Callbacks{
                OnError: func(err error) {
                        errorReceived = true
                },
        })

        err := a.Run(context.Background(), "test")
        if err == nil {
                t.Fatal("expected error, got nil")
        }

        if !errorReceived {
                t.Error("expected OnError callback to fire for provider error")
        }
}

// TestAgentErrorCallbackStreaming verifies OnError fires for streaming
// provider errors.
func TestAgentErrorCallbackStreaming(t *testing.T) {
        provider := &mockStreamingProvider{
                mockProvider: &mockProvider{name: "mock-stream"},
                streamFn: func(ctx context.Context, messages []llm.Message, toolDefs []llm.ToolDefinition, system string, model string, cb llm.StreamingCallback) (*llm.Response, error) {
                        return nil, errors.New("stream connection reset")
                },
        }

        cfg := &config.Config{DefaultModel: "mock-model", MaxTurns: 10}
        a := NewAgent(provider, tools.NewRegistry(), cfg, &tools.TodoStore{})

        var errorReceived bool
        a.SetCallbacks(Callbacks{
                OnError: func(err error) {
                        errorReceived = true
                },
        })

        err := a.Run(context.Background(), "test")
        if err == nil {
                t.Fatal("expected error, got nil")
        }

        if !errorReceived {
                t.Error("expected OnError callback to fire for streaming provider error")
        }
}

// TestStreamingDrainLoopTermination verifies the drain loop pattern used in
// repl.go terminates correctly when chunkCh is closed (simulates the
// labeled-break fix).
func TestStreamingDrainLoopTermination(t *testing.T) {
        chunkCh := make(chan string, 256)
        resultCh := make(chan string, 1)

        // Simulate goroutine: send chunks, then send result, then close
        go func() {
                chunkCh <- "hello"
                chunkCh <- " "
                chunkCh <- "world"
                // Small delay to ensure chunks are buffered before close
                time.Sleep(10 * time.Millisecond)
                resultCh <- "done"
                close(chunkCh)
        }()

        // Small delay to let goroutine send all chunks
        time.Sleep(20 * time.Millisecond)

        // Simulate drain loop (same pattern as repl.go after fix)
        var accumulated string
        var result string

drainLoop:
        for {
                select {
                case chunk, ok := <-chunkCh:
                        if !ok {
                                // Channel closed — read result
                                result = <-resultCh
                                break drainLoop
                        }
                        accumulated += chunk
                default:
                        break drainLoop
                }
        }

        if accumulated != "hello world" {
                t.Errorf("expected 'hello world', got '%s'", accumulated)
        }
        if result != "done" {
                t.Errorf("expected result 'done', got '%s'", result)
        }
}

// TestStreamingDrainPanicSafety verifies that even if the goroutine panics
// before sending to resultCh, the drain loop doesn't hang forever.
func TestStreamingDrainPanicSafety(t *testing.T) {
        chunkCh := make(chan string, 256)

        // Simulate goroutine that panics
        go func() {
                defer func() {
                        recover()
                        close(chunkCh)
                }()
                panic("something went wrong")
        }()

        // Simulate drain loop — should terminate when chunkCh closes
        terminated := false
        timeout := time.After(2 * time.Second)

drainLoop:
        for {
                select {
                case _, ok := <-chunkCh:
                        if !ok {
                                terminated = true
                                break drainLoop
                        }
                case <-timeout:
                        t.Fatal("drain loop hung — chunkCh was never closed (panic safety failed)")
                }
        }

        if !terminated {
                t.Error("drain loop did not terminate")
        }
}
