package ui

import (
        "context"
        "encoding/json"
        "fmt"
        "html"
        "regexp"
        "strings"
        "time"

        "github.com/charmbracelet/bubbletea"
        "github.com/charmbracelet/glamour"
        "github.com/charmbracelet/lipgloss"

        "github.com/cairn/cairn-code/internal/agent"
        "github.com/cairn/cairn-code/internal/llm"
        "github.com/cairn/cairn-code/internal/session"
)

// State represents the REPL state.
type state int

const (
        stateIdle state = iota
        stateRunning
)

// OutputLine represents a line of output from the agent.
type OutputLine struct {
        Type     string // "text", "tool_use", "tool_result", "error", "system"
        Content  string
        ToolName string
        Duration time.Duration
}

// replModel is the bubbletea Model for the terminal REPL.
type replModel struct {
        agent      *agent.Agent
        state      state
        input      string
        cursor     int
        output     []OutputLine
        history    []string
        histIdx    int
        width      int
        height     int
        totalUsage llm.Usage
        err        error
        quit       bool
        renderer   *glamour.TermRenderer
        spinner    int
        sessionDir string
        sessionID  string // current session ID for auto-save
        scrollY    int    // current scroll offset (0 = bottom, newest)
        maxViewY   int    // total rendered height of output
        atBottom   bool   // whether viewport is at the bottom
}

var (
        // Styles
        promptStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("63")) // cyan-ish

        userStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("221")) // warm yellow

        toolNameStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("6")) // cyan

        toolResultStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245")) // dim

        errorStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("196")) // red

        systemStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245")) // dim

        usageStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245")) // dim

        titleStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("63"))

        spinnerChars = []string{"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"}
)

// NewREPL creates a new REPL model.
func NewREPL(a *agent.Agent, sessionDir string) *replModel {
        renderer, err := glamour.NewTermRenderer(
                glamour.WithAutoStyle(),
                glamour.WithEmoji(),
        )
        if err != nil {
                renderer = nil
        }

        return &replModel{
                agent:      a,
                state:      stateIdle,
                histIdx:    -1,
                renderer:   renderer,
                sessionDir: sessionDir,
                atBottom:   true,
        }
}

// Init initializes the model.
func (m *replModel) Init() tea.Cmd {
        return tickSpinner()
}

// Update handles messages.
func (m *replModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
        switch msg := msg.(type) {
        case tea.WindowSizeMsg:
                m.width = msg.Width
                m.height = msg.Height
                return m, nil

        case tea.MouseMsg:
                switch msg.Type {
                case tea.MouseWheelUp:
                        m.scrollUp(3)
                case tea.MouseWheelDown:
                        m.scrollDown(3)
                }

        case tea.KeyMsg:
                switch msg.String() {
                case "ctrl+c":
                        if m.state == stateRunning {
                                // Request cancellation - will be handled by context
                                m.quit = true
                                return m, tea.Quit
                        }
                        m.quit = true
                        return m, tea.Quit

                // Scrolling keys
                case "pgup", "shift+up":
                        m.scrollUp(m.height / 2)
                case "pgdown", "shift+down":
                        m.scrollDown(m.height / 2)
                case "home":
                        m.scrollY = m.maxViewY
                        m.atBottom = false
                case "end":
                        m.scrollY = 0
                        m.atBottom = true

                case "enter":
                        if m.state == stateRunning {
                                return m, nil
                        }

                        input := strings.TrimSpace(m.input)
                        m.input = ""

                        // Handle commands
                        if strings.HasPrefix(input, "/") {
                                return m.handleCommand(input)
                        }

                        if input == "" {
                                return m, nil
                        }

                        // Add to history
                        m.history = append(m.history, input)
                        m.histIdx = len(m.history)

                        // Add user message to output
                        m.output = append(m.output, OutputLine{
                                Type:    "user",
                                Content: input,
                        })

                        // Run agent (with spinner)
                        m.state = stateRunning
                        return m, tea.Batch(m.runAgent(input), tickSpinner())

                case "up":
                        if m.histIdx > 0 {
                                m.histIdx--
                                m.input = m.history[m.histIdx]
                        } else if m.histIdx == 0 {
                                m.input = m.history[0]
                        }

                case "down":
                        if m.histIdx < len(m.history)-1 {
                                m.histIdx++
                                m.input = m.history[m.histIdx]
                        } else {
                                m.histIdx = len(m.history)
                                m.input = ""
                        }

                case "backspace", "ctrl+h", "ctrl+?":
                        if m.cursor > 0 && m.cursor <= len(m.input) {
                                m.input = m.input[:m.cursor-1] + m.input[m.cursor:]
                                m.cursor--
                        }

                case "delete":
                        if m.cursor < len(m.input) {
                                m.input = m.input[:m.cursor] + m.input[m.cursor+1:]
                        }

                case "ctrl+u":
                        m.input = m.input[m.cursor:]
                        m.cursor = 0

                case "ctrl+w":
                        // Delete word before cursor
                        if m.cursor > 0 {
                                i := m.cursor - 1
                                for i > 0 && m.input[i-1] != ' ' {
                                        i--
                                }
                                m.input = m.input[:i] + m.input[m.cursor:]
                                m.cursor = i
                        }

                case "left":
                        if m.cursor > 0 {
                                m.cursor--
                        }

                case "right":
                        if m.cursor < len(m.input) {
                                m.cursor++
                        }

                default:
                        // Insert character at cursor position (skip control chars)
                        if len(msg.String()) == 1 && msg.String()[0] >= 32 {
                                if m.cursor < len(m.input) {
                                        m.input = m.input[:m.cursor] + msg.String() + m.input[m.cursor:]
                                } else {
                                        m.input += msg.String()
                                }
                                m.cursor++
                        }
                }

        case agentCompleteMsg:
                m.state = stateIdle
                m.output = append(m.output, msg.output...)
                m.totalUsage.InputTokens += msg.usage.InputTokens
                m.totalUsage.OutputTokens += msg.usage.OutputTokens
                m.totalUsage.CacheRead += msg.usage.CacheRead
                m.totalUsage.CacheCreate += msg.usage.CacheCreate
                if msg.err != nil {
                        m.err = msg.err
                }
                // Auto-save session after each agent run
                if len(m.agent.History()) > 0 {
                        m.autoSaveSession()
                }

        case agentResultMsg:
                m.state = stateIdle
                if msg.err != nil {
                        m.output = append(m.output, OutputLine{
                                Type:    "error",
                                Content: msg.err.Error(),
                        })
                        m.err = msg.err
                }

        case agentTextMsg:
                m.output = append(m.output, OutputLine{
                        Type:    "text",
                        Content: msg.text,
                })

        case agentToolUseMsg:
                m.output = append(m.output, OutputLine{
                        Type:     "tool_use",
                        ToolName: msg.name,
                        Content:  formatToolInput(msg.input),
                })

        case agentToolResultMsg:
                m.output = append(m.output, OutputLine{
                        Type:     "tool_result",
                        ToolName: msg.name,
                        Content:  msg.output,
                        Duration: msg.duration,
                })

        case agentTurnEndMsg:
                m.totalUsage.InputTokens += msg.usage.InputTokens
                m.totalUsage.OutputTokens += msg.usage.OutputTokens
                m.totalUsage.CacheRead += msg.usage.CacheRead
                m.totalUsage.CacheCreate += msg.usage.CacheCreate

        case spinnerTickMsg:
                m.spinner = (m.spinner + 1) % len(spinnerChars)
                if m.state == stateRunning {
                        return m, tickSpinner()
                }
                return m, nil
        }

        return m, nil
}

// View renders the model.
func (m replModel) View() string {
        if m.quit && m.err == nil {
                return ""
        }

        // Render all output into full content
        var content strings.Builder

        // Title
        content.WriteString(titleStyle.Render("⚡ Cairn Code"))
        if m.agent != nil {
                content.WriteString(systemStyle.Render(fmt.Sprintf("  [%s / %s]", m.agent.ProviderName(), m.agent.Model())))
        }
        if m.sessionID != "" {
                content.WriteString(systemStyle.Render(fmt.Sprintf("  session: %s", m.sessionID[:8])))
        }
        content.WriteString("\n\n")

        // Output
        for _, line := range m.output {
                content.WriteString(m.renderOutputLine(line))
        }

        // Spinner if running
        if m.state == stateRunning {
                content.WriteString(fmt.Sprintf("%s Thinking...", spinnerChars[m.spinner]))
        }

        // Usage summary
        if m.totalUsage.InputTokens > 0 {
                content.WriteString(usageStyle.Render(fmt.Sprintf(
                        "\nTokens: %d in, %d out",
                        m.totalUsage.InputTokens,
                        m.totalUsage.OutputTokens,
                )))
                content.WriteString("\n")
        }

        fullContent := content.String()
        contentLines := strings.Split(fullContent, "\n")
        // Remove trailing empty line from final newline
        if len(contentLines) > 0 && contentLines[len(contentLines)-1] == "" {
                contentLines = contentLines[:len(contentLines)-1]
        }

        // Calculate viewport height (leave room for header line + input line + padding)
        viewportHeight := m.height - 4
        if viewportHeight < 1 {
                viewportHeight = 1
        }

        totalHeight := len(contentLines)

        // Auto-scroll to bottom when new output arrives and user is at bottom
        if m.atBottom || m.scrollY < 0 {
                m.scrollY = 0
                m.atBottom = true
        }

        // Clamp scrollY
        maxScroll := totalHeight - viewportHeight
        if maxScroll < 0 {
                maxScroll = 0
        }
        if m.scrollY > maxScroll {
                m.scrollY = maxScroll
        }
        m.maxViewY = maxScroll

        // Determine visible window
        // scrollY=0 means bottom (newest), scrollY=maxScroll means top (oldest)
        startIdx := totalHeight - viewportHeight - m.scrollY
        if startIdx < 0 {
                startIdx = 0
        }
        endIdx := startIdx + viewportHeight
        if endIdx > totalHeight {
                endIdx = totalHeight
        }

        // Build visible content (no custom scrollbar)
        var b strings.Builder
        for i := startIdx; i < endIdx; i++ {
                b.WriteString(contentLines[i])
                if i < endIdx-1 {
                        b.WriteString("\n")
                }
        }

        // Input prompt (always visible at bottom)
        if !m.quit {
                b.WriteString("\n")
                b.WriteString(promptStyle.Render("⟩ "))
                b.WriteString(m.input)
        }

        return b.String()
}

// scrollUp moves the viewport toward older content (increases scrollY).
func (m *replModel) scrollUp(amount int) {
        m.scrollY += amount
        m.atBottom = false
}

// scrollDown moves the viewport toward newer content (decreases scrollY).
func (m *replModel) scrollDown(amount int) {
        m.scrollY -= amount
        if m.scrollY <= 0 {
                m.scrollY = 0
                m.atBottom = true
        }
}

// renderOutputLine renders a single output line.
func (m *replModel) renderOutputLine(line OutputLine) string {
        switch line.Type {
        case "user":
                return userStyle.Render("⟩ " + line.Content) + "\n\n"

        case "text":
                rendered := line.Content
                if m.renderer != nil {
                        clean := stripHTMLTags(line.Content)
                        md, err := m.renderer.Render(clean)
                        if err == nil {
                                rendered = md
                        }
                }
                return rendered + "\n"

        case "tool_use":
                var b strings.Builder
                b.WriteString(toolNameStyle.Render(fmt.Sprintf("▸ %s", line.ToolName)))
                if line.Content != "" {
                        b.WriteString("\n")
                        // Truncate long tool inputs for display
                        if len(line.Content) > 500 {
                                b.WriteString(toolResultStyle.Render(line.Content[:500] + "..."))
                        } else {
                                b.WriteString(toolResultStyle.Render(line.Content))
                        }
                }
                b.WriteString("\n")
                return b.String()

        case "tool_result":
                var b strings.Builder
                b.WriteString(toolResultStyle.Render(fmt.Sprintf("  ✓ %s", line.ToolName)))
                if line.Duration > 0 {
                        b.WriteString(usageStyle.Render(fmt.Sprintf(" (%.1fs)", line.Duration.Seconds())))
                }
                b.WriteString("\n")
                // Truncate long tool results for display
                content := strings.TrimSpace(line.Content)
                if len(content) > 2000 {
                        content = content[:2000] + "\n... [output truncated]"
                }
                if content != "" {
                        b.WriteString(toolResultStyle.Render(indent(content, "    ")))
                        b.WriteString("\n")
                }
                return b.String()

        case "error":
                return errorStyle.Render("✗ " + line.Content) + "\n\n"

        case "system":
                return systemStyle.Render(line.Content) + "\n"

        default:
                return line.Content + "\n"
        }
}

// handleCommand processes slash commands.
func (m *replModel) handleCommand(cmd string) (tea.Model, tea.Cmd) {
        parts := strings.Fields(cmd)
        if len(parts) == 0 {
                return m, nil
        }

        switch parts[0] {
        case "/help", "/?":
                helpText := `
Available commands:
  /help              Show this help message
  /clear             Clear conversation history
  /model [name]      Show or change the current model
  /compact           Summarize and compact the conversation using the LLM
  /cost              Show token usage summary
  /provider          Show current provider
  /save              Save current session to disk
  /resume [id]       Resume a saved session (latest if no ID given)
  /sessions          List all saved sessions
  /tools             List available tools
  /quit, /exit       Exit the application
`
                m.output = append(m.output, OutputLine{
                        Type:    "system",
                        Content: helpText,
                })
                return m, nil

        case "/clear":
                m.agent.Reset()
                m.totalUsage = llm.Usage{}
                m.sessionID = ""
                m.output = append(m.output, OutputLine{
                        Type:    "system",
                        Content: "Conversation cleared.",
                })
                return m, nil

        case "/compact":
                m.state = stateRunning
                return m, tea.Batch(m.runCompact(), tickSpinner())

        case "/model":
                if len(parts) > 1 {
                        newModel := strings.Join(parts[1:], " ")
                        m.agent.SetModel(newModel)
                        m.output = append(m.output, OutputLine{
                                Type:    "system",
                                Content: fmt.Sprintf("Model set to: %s", newModel),
                        })
                } else {
                        models := m.agent.ProviderName()
                        m.output = append(m.output, OutputLine{
                                Type:    "system",
                                Content: fmt.Sprintf("Current model: %s (provider: %s)", m.agent.Model(), models),
                        })
                }
                return m, nil

        case "/cost":
                cost := fmt.Sprintf("Token usage:\n  Input: %d\n  Output: %d", m.totalUsage.InputTokens, m.totalUsage.OutputTokens)
                m.output = append(m.output, OutputLine{
                        Type:    "system",
                        Content: cost,
                })
                return m, nil

        case "/provider":
                m.output = append(m.output, OutputLine{
                        Type:    "system",
                        Content: fmt.Sprintf("Provider: %s", m.agent.ProviderName()),
                })
                return m, nil

        case "/save":
                return m, m.saveCurrentSession()

        case "/resume":
                resumeID := ""
                if len(parts) > 1 {
                        resumeID = parts[1]
                }
                m.state = stateRunning
                return m, tea.Batch(m.resumeSession(resumeID), tickSpinner())

        case "/sessions":
                m.state = stateRunning
                return m, tea.Batch(m.listSessions(), tickSpinner())

        case "/tools":
                m.output = append(m.output, OutputLine{
                        Type:    "system",
                        Content: "Available tools: file_read, file_write, file_edit, bash, glob, grep, todo_write, web_search, web_fetch",
                })
                return m, nil

        case "/quit", "/exit", "/q":
                m.quit = true
                return m, tea.Quit

        default:
                m.output = append(m.output, OutputLine{
                        Type:    "error",
                        Content: fmt.Sprintf("Unknown command: %s (type /help for available commands)", parts[0]),
                })
                return m, nil
        }
}

// runAgent runs the agent in a goroutine and returns commands to the tea runtime.
func (m replModel) runAgent(input string) tea.Cmd {
        return func() tea.Msg {
                // Collect output via callbacks
                var collectedOutput []OutputLine
                var totalUsage llm.Usage
                var agentErr error

                cb := agent.Callbacks{
                        OnText: func(text string) {
                                collectedOutput = append(collectedOutput, OutputLine{
                                        Type:    "text",
                                        Content: text,
                                })
                        },
                        OnToolUse: func(name string, input any) {
                                collectedOutput = append(collectedOutput, OutputLine{
                                        Type:     "tool_use",
                                        ToolName: name,
                                        Content:  formatToolInput(input),
                                })
                        },
                        OnToolResult: func(name string, output string, duration time.Duration) {
                                collectedOutput = append(collectedOutput, OutputLine{
                                        Type:     "tool_result",
                                        ToolName: name,
                                        Content:  output,
                                        Duration: duration,
                                })
                        },
                        OnTurnEnd: func(turn int, usage llm.Usage) {
                                totalUsage.InputTokens += usage.InputTokens
                                totalUsage.OutputTokens += usage.OutputTokens
                                totalUsage.CacheRead += usage.CacheRead
                                totalUsage.CacheCreate += usage.CacheCreate
                        },
                        OnError: func(err error) {
                                collectedOutput = append(collectedOutput, OutputLine{
                                        Type:    "error",
                                        Content: err.Error(),
                                })
                        },
                        OnPermission: func(tool string, input any) bool {
                                return true
                        },
                }

                a := m.agent
                a.SetCallbacks(cb)
                agentErr = a.Run(context.Background(), input)

                return agentCompleteMsg{
                        output: collectedOutput,
                        usage:  totalUsage,
                        err:    agentErr,
                }
        }
}

// runCompact runs the compaction command.
func (m replModel) runCompact() tea.Cmd {
        return func() tea.Msg {
                a := m.agent
                a.SetCallbacks(agent.Callbacks{
                        OnError: func(err error) {
                                // noop — handled below
                        },
                })

                err := a.Compact(context.Background())
                if err != nil {
                        return agentCompleteMsg{
                                output: []OutputLine{
                                        {Type: "error", Content: fmt.Sprintf("Compaction failed: %v", err)},
                                },
                                usage: llm.Usage{},
                                err:   err,
                        }
                }

                return agentCompleteMsg{
                        output: []OutputLine{
                                {Type: "system", Content: "Conversation compacted successfully."},
                        },
                        usage: llm.Usage{},
                }
        }
}

// saveCurrentSession saves the current conversation as a session.
func (m replModel) saveCurrentSession() tea.Cmd {
        return func() tea.Msg {
                history := m.agent.History()
                if len(history) == 0 {
                        return agentCompleteMsg{
                                output: []OutputLine{
                                        {Type: "system", Content: "Nothing to save — conversation is empty."},
                                },
                        }
                }

                id := session.NewSessionID()
                sess := session.FromMessages(id, history, m.agent.Model(), m.agent.ProviderName(),
                        m.totalUsage.InputTokens, m.totalUsage.OutputTokens)

                if err := session.SaveSession(m.sessionDir, sess); err != nil {
                        return agentCompleteMsg{
                                output: []OutputLine{
                                        {Type: "error", Content: fmt.Sprintf("Failed to save session: %v", err)},
                                },
                        }
                }

                m.sessionID = id
                return agentCompleteMsg{
                        output: []OutputLine{
                                {Type: "system", Content: fmt.Sprintf("Session saved: %s", id)},
                        },
                }
        }
}

// resumeSession loads and resumes a saved session.
func (m replModel) resumeSession(id string) tea.Cmd {
        return func() tea.Msg {
                sessDir := m.sessionDir
                if id == "" {
                        // Load the most recent session
                        sessions, err := session.ListSessions(sessDir)
                        if err != nil {
                                return agentCompleteMsg{
                                        output: []OutputLine{
                                                {Type: "error", Content: fmt.Sprintf("Failed to list sessions: %v", err)},
                                        },
                                }
                        }
                        if len(sessions) == 0 {
                                return agentCompleteMsg{
                                        output: []OutputLine{
                                                {Type: "system", Content: "No saved sessions found."},
                                        },
                                }
                        }
                        id = sessions[0].ID
                }

                sess, err := session.LoadSession(sessDir, id)
                if err != nil {
                        return agentCompleteMsg{
                                output: []OutputLine{
                                        {Type: "error", Content: fmt.Sprintf("Failed to load session %s: %v", id, err)},
                                },
                        }
                }

                // Restore state
                if sess.Model != "" {
                        m.agent.SetModel(sess.Model)
                }
                m.agent.SetMessages(sess.ToMessages())
                m.totalUsage = llm.Usage{
                        InputTokens:  sess.TokensIn,
                        OutputTokens: sess.TokensOut,
                }
                m.sessionID = sess.ID

                return agentCompleteMsg{
                        output: []OutputLine{
                                {Type: "system", Content: fmt.Sprintf("Resumed session %s (model: %s, messages: %d)", sess.ID, sess.Model, len(sess.Messages))},
                        },
                }
        }
}

// listSessions lists all saved sessions.
func (m replModel) listSessions() tea.Cmd {
        return func() tea.Msg {
                sessions, err := session.ListSessions(m.sessionDir)
                if err != nil {
                        return agentCompleteMsg{
                                output: []OutputLine{
                                        {Type: "error", Content: fmt.Sprintf("Failed to list sessions: %v", err)},
                                },
                        }
                }

                if len(sessions) == 0 {
                        return agentCompleteMsg{
                                output: []OutputLine{
                                        {Type: "system", Content: "No saved sessions found."},
                                },
                        }
                }

                var buf strings.Builder
                buf.WriteString(fmt.Sprintf("Saved sessions (%d):\n\n", len(sessions)))
                for i, s := range sessions {
                        summary := s.Summary
                        if summary == "" {
                                summary = "(no summary)"
                        }
                        buf.WriteString(fmt.Sprintf("  %d. %s  model=%s  msgs=%d  updated=%s\n", i+1, s.ID[:8], s.Model, len(s.Messages), s.UpdatedAt.Format("2006-01-02 15:04")))
                        if len(summary) > 60 {
                                summary = summary[:60] + "..."
                        }
                        buf.WriteString(fmt.Sprintf("     %s\n", summary))
                }

                return agentCompleteMsg{
                        output: []OutputLine{
                                {Type: "system", Content: buf.String()},
                        },
                }
        }
}

// autoSaveSession saves the current session if there is one.
func (m *replModel) autoSaveSession() {
        history := m.agent.History()
        if len(history) == 0 {
                return
        }

        id := m.sessionID
        if id == "" {
                id = session.NewSessionID()
                m.sessionID = id
        }

        sess := session.FromMessages(id, history, m.agent.Model(), m.agent.ProviderName(),
                m.totalUsage.InputTokens, m.totalUsage.OutputTokens)
        // Preserve created-at from existing session
        if prev, err := session.LoadSession(m.sessionDir, id); err == nil {
                sess.CreatedAt = prev.CreatedAt
        }

        _ = session.SaveSession(m.sessionDir, sess)
}

// Message types for tea.Cmd communication.
type agentResultMsg struct {
        err error
}

type agentTextMsg struct {
        text string
}

type agentToolUseMsg struct {
        name  string
        input any
}

type agentToolResultMsg struct {
        name     string
        output   string
        duration time.Duration
}

type agentTurnEndMsg struct {
        usage llm.Usage
}

type spinnerTickMsg time.Time

type agentCompleteMsg struct {
        output []OutputLine
        usage  llm.Usage
        err    error
}

// tickSpinner returns a command that ticks the spinner.
func tickSpinner() tea.Cmd {
        return tea.Tick(time.Millisecond*80, func(t time.Time) tea.Msg {
                return spinnerTickMsg(t)
        })
}

// formatToolInput formats a tool's input for display.
func formatToolInput(input any) string {
        if input == nil {
                return ""
        }
        data, err := json.Marshal(input)
        if err != nil {
                return fmt.Sprintf("%v", input)
        }
        // Pretty-print the JSON
        var pretty map[string]any
        if err := json.Unmarshal(data, &pretty); err == nil {
                data, err = json.MarshalIndent(pretty, "", "  ")
                if err == nil {
                        return string(data)
                }
        }
        return string(data)
}

// indent indents each line of text with the given prefix.
func indent(text, prefix string) string {
        lines := strings.Split(text, "\n")
        for i, line := range lines {
                lines[i] = prefix + line
        }
        return strings.Join(lines, "\n")
}

// htmlTagRe matches HTML tags for stripping.
var htmlTagRe = regexp.MustCompile(`<[^>]*>`)

// stripHTMLTags removes HTML tags from text, replacing common block elements
// with newlines to preserve some structure.
func stripHTMLTags(s string) string {
        // Replace block-level closing tags with newlines
        s = strings.ReplaceAll(s, "</p>", "\n")
        s = strings.ReplaceAll(s, "</div>", "\n")
        s = strings.ReplaceAll(s, "</li>", "\n")
        s = strings.ReplaceAll(s, "<br>", "\n")
        s = strings.ReplaceAll(s, "<br/>", "\n")
        s = strings.ReplaceAll(s, "<br />", "\n")
        s = strings.ReplaceAll(s, "</h1>", "\n")
        s = strings.ReplaceAll(s, "</h2>", "\n")
        s = strings.ReplaceAll(s, "</h3>", "\n")
        s = strings.ReplaceAll(s, "</h4>", "\n")
        s = strings.ReplaceAll(s, "</tr>", "\n")
        // Replace <hr> with a divider
        s = strings.ReplaceAll(s, "<hr>", "\n---\n")
        s = strings.ReplaceAll(s, "<hr/>", "\n---\n")
        // Strip all remaining HTML tags
        s = htmlTagRe.ReplaceAllString(s, "")
        // Unescape HTML entities
        s = html.UnescapeString(s)
        return s
}
