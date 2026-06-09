package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"syscall"
	"time"

	tea "github.com/charmbracelet/bubbletea"

	"github.com/cairn/cairn-code/internal/agent"
	"github.com/cairn/cairn-code/internal/config"
	"github.com/cairn/cairn-code/internal/llm"
	"github.com/cairn/cairn-code/internal/session"
	"github.com/cairn/cairn-code/internal/tools"
	"github.com/cairn/cairn-code/internal/ui"
)

const version = "0.2.0"

func main() {
	// Parse flags
	printMode := flag.Bool("p", false, "Print mode: run prompt non-interactively, print result, and exit")
	modelFlag := flag.String("model", "", "Override the default model")
	providerFlag := flag.String("provider", "", "Override the default provider")
	showVersion := flag.Bool("version", false, "Print version and exit")
	resumeFlag := flag.String("resume", "", "Resume a session by ID")
	sessionDir := flag.String("session-dir", "", "Custom directory for session storage")
	flag.Parse()

	if *showVersion {
		fmt.Printf("cairn-code version %s\n", version)
		os.Exit(0)
	}

	// Load configuration
	cfg, err := config.LoadConfig()
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error loading config: %v\n", err)
		os.Exit(1)
	}

	// Override config with flags
	if *providerFlag != "" {
		cfg.DefaultProvider = *providerFlag
	}
	if *modelFlag != "" {
		cfg.DefaultModel = *modelFlag
	}

	// Create LLM provider
	provider, err := llm.NewProvider(cfg)
	if err != nil {
		fmt.Fprintf(os.Stderr, "Error creating provider: %v\n", err)
		os.Exit(1)
	}

	// Create tool registry and register tools
	registry := tools.NewRegistry()
	todoStore := &tools.TodoStore{}

	registry.Register(tools.NewFileReadTool())
	registry.Register(tools.NewFileWriteTool())
	registry.Register(tools.NewFileEditTool())
	registry.Register(tools.NewBashTool())
	registry.Register(tools.NewGlobTool())
	registry.Register(tools.NewGrepTool())
	registry.Register(tools.NewTodoWriteTool(todoStore))
	registry.Register(tools.NewWebSearchTool())
	registry.Register(tools.NewWebFetchTool())

	// Create agent
	ag := agent.NewAgent(provider, registry, cfg, todoStore)

	// Resolve session directory
	sessDir := *sessionDir
	if sessDir == "" {
		sessDir = session.DefaultSessionDir()
	}

	// Handle session resume
	if *resumeFlag != "" {
		sess, err := session.LoadSession(sessDir, *resumeFlag)
		if err != nil {
			fmt.Fprintf(os.Stderr, "Error loading session %s: %v\n", *resumeFlag, err)
			os.Exit(1)
		}

		// Restore model and messages
		if sess.Model != "" {
			ag.SetModel(sess.Model)
		}
		ag.SetMessages(sess.ToMessages())
		fmt.Fprintf(os.Stderr, "Resumed session %s (model: %s, messages: %d)\n", sess.ID, sess.Model, len(sess.Messages))
	}

	// Handle SIGINT gracefully
	sigChan := make(chan os.Signal, 1)
	signal.Notify(sigChan, syscall.SIGINT)
	go func() {
		<-sigChan
		fmt.Fprintf(os.Stderr, "\nInterrupted. Goodbye!\n")
		os.Exit(130)
	}()

	// Get prompt
	args := flag.Args()
	prompt := ""
	if len(args) > 0 {
		prompt = args[0]
	}

	// Print mode
	if *printMode {
		if prompt == "" {
			fmt.Fprintf(os.Stderr, "Error: prompt required in print mode (-p)\n")
			fmt.Fprintf(os.Stderr, "Usage: cairn-code -p \"your prompt\"\n")
			os.Exit(1)
		}

		// Set up print mode callbacks
		ag.SetCallbacks(agent.Callbacks{
			OnText: func(text string) {
				fmt.Print(text)
			},
			OnToolUse: func(name string, input any) {
				fmt.Fprintf(os.Stderr, "▸ %s\n", name)
			},
			OnToolResult: func(name string, output string, duration time.Duration) {
				fmt.Fprintf(os.Stderr, "  ✓ %s (%.1fs)\n", name, duration.Seconds())
			},
			OnPermission: func(tool string, input any) bool {
				return true // Auto-allow in print mode
			},
			OnError: func(err error) {
				fmt.Fprintf(os.Stderr, "Error: %v\n", err)
			},
		})

		// Run agent
		if err := ag.Run(context.Background(), prompt); err != nil {
			fmt.Fprintf(os.Stderr, "Error: %v\n", err)
			os.Exit(1)
		}

		// Auto-save session in print mode
		if len(ag.History()) > 0 {
			sessID := session.NewSessionID()
			sess := session.FromMessages(sessID, ag.History(), ag.Model(), ag.ProviderName(), 0, 0)
			if err := session.SaveSession(sessDir, sess); err != nil {
				fmt.Fprintf(os.Stderr, "Warning: could not save session: %v\n", err)
			}
		}

		return
	}

	// Interactive REPL mode — pass session dir to the REPL
	p := tea.NewProgram(ui.NewREPL(ag, sessDir), tea.WithAltScreen())
	if _, err := p.Run(); err != nil {
		fmt.Fprintf(os.Stderr, "Error: %v\n", err)
		os.Exit(1)
	}
}
