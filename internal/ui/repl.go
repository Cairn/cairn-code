package ui

import (
        "context"
        "fmt"
        "strings"
        "sync"
        "time"

        "github.com/charmbracelet/bubbles/viewport"
        tea "github.com/charmbracelet/bubbletea"
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
        Type     string // "text", "tool_use", "tool_result", "error", "system", "user"
        Content  string
        ToolName string
        Duration time.Duration
}

// streamEvent is sent from the agent goroutine to the UI via a channel.
type streamEvent struct {
        typ        string
        text       string
        toolName   string
        toolInput  any
        toolOutput string
        duration   time.Duration
        usage      llm.Usage
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
        sessionID  string
        version    string

        // Viewport scrolling
        vp             viewport.Model
        userScrolledUp bool
        contentDirty   bool   // true when output has changed and needs re-render
        cachedContent  string // rendered content cache

        // Real-time streaming state
        streamCh   chan streamEvent
        resultCh   chan agentResult
        streamText string
        activeTool string
        drainDone  bool
        mu         *sync.Mutex
}

var (
        promptStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("63"))

        userStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("221"))

        toolNameStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("6"))

        toolResultStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245"))

        toolSuccessStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("82"))

        toolErrorStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("196"))

        errorStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("196"))

        systemStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245"))

        usageStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245"))

        titleStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("63"))

        spinnerStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("63"))

        activityStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("243"))

        scrollHintStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("243"))

        spinnerChars = []string{"\u280B", "\u2819", "\u2839", "\u2838", "\u283C", "\u2834", "\u2826", "\u2827", "\u2807", "\u280F"}
)

// NewREPL creates a new REPL model.
func NewREPL(a *agent.Agent, sessionDir string, version string) replModel {
        renderer, err := glamour.NewTermRenderer(
                glamour.WithAutoStyle(),
                glamour.WithEmoji(),
        )
        if err != nil {
                renderer = nil
        }

        vp := viewport.New(80, 20)
        vp.MouseWheelEnabled = true
        vp.MouseWheelDelta = 3

        return replModel{
                agent:         a,
                state:         stateIdle,
                histIdx:       -1,
                renderer:      renderer,
                sessionDir:    sessionDir,
                version:       version,
                vp:            vp,
                streamCh:      make(chan streamEvent, 256),
                resultCh:      make(chan agentResult, 1),
                drainDone:     true,
                contentDirty:  true,
                mu:            &sync.Mutex{},
        }
}

// Init initializes the model.
func (m replModel) Init() tea.Cmd {
        return tea.Batch(tickSpinner(), m.vp.Init())
}

// markDirty marks the content as needing re-render.
func (m *replModel) markDirty() {
        m.contentDirty = true
}

// ensureContent renders output to the viewport if dirty.
func (m *replModel) ensureContent() {
        if !m.contentDirty {
                return
        }
        m.contentDirty = false

        var b strings.Builder
        for _, line := range m.output {
                b.WriteString(m.renderOutputLine(line))
        }
        m.cachedContent = b.String()
        m.vp.SetContent(m.cachedContent)
}

// rebuildViewportContent rebuilds viewport content including live streaming text.
// Used during active streaming for the spinner + cursor overlay.
func (m *replModel) rebuildViewportContent() {
        var b strings.Builder
        b.WriteString(m.cachedContent)

        // Append live streaming text
        m.mu.Lock()
        streamingText := m.streamText
        m.mu.Unlock()

        if streamingText != "" {
                lines := strings.Split(streamingText, "\n")
                var completeLines []string
                lastLine := ""
                if len(lines) > 0 {
                        if strings.HasSuffix(streamingText, "\n") {
                                completeLines = lines
                                if len(completeLines) > 0 && completeLines[len(completeLines)-1] == "" {
                                        completeLines = completeLines[:len(completeLines)-1]
                                }
                        } else {
                                completeLines = lines[:len(lines)-1]
                                lastLine = lines[len(lines)-1]
                        }
                }

                if len(completeLines) > 0 {
                        block := strings.Join(completeLines, "\n")
                        if m.renderer != nil {
                                rendered, err := m.renderer.Render(block)
                                if err == nil {
                                        b.WriteString(rendered)
                                } else {
                                        b.WriteString(block)
                                }
                        } else {
                                b.WriteString(block)
                        }
                }

                if lastLine != "" {
                        b.WriteString(lastLine)
                }
                b.WriteString("\u258C\n")
        }

        m.vp.SetContent(b.String())
}

// Update handles messages.
func (m replModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
        var cmds []tea.Cmd

        switch msg := msg.(type) {
        case tea.WindowSizeMsg:
                m.width = msg.Width
                m.height = msg.Height
                headerH := 2
                footerH := 4
                vpH := msg.Height - headerH - footerH
                if vpH < 3 {
                        vpH = 3
                }
                m.vp = viewport.New(msg.Width, vpH)
                m.vp.MouseWheelEnabled = true
                m.vp.MouseWheelDelta = 3
                m.contentDirty = true
                m.ensureContent()
                m.vp.GotoBottom()
                return m, nil

        case tea.KeyMsg:
                // Scroll keys when user has scrolled up
                if m.userScrolledUp && m.state == stateIdle {
                        switch msg.String() {
                        case "up", "k":
                                m.vp.LineUp(1)
                                return m, nil
                        case "down", "j":
                                if m.vp.AtBottom() {
                                        m.userScrolledUp = false
                                } else {
                                        m.vp.LineDown(1)
                                }
                                return m, nil
                        case "pgup":
                                m.vp.HalfPageUp()
                                return m, nil
                        case "pgdown", " ":
                                if m.vp.PastBottom() {
                                        m.vp.GotoBottom()
                                        m.userScrolledUp = false
                                } else {
                                        m.vp.HalfPageDown()
                                }
                                return m, nil
                        case "home":
                                m.vp.GotoTop()
                                return m, nil
                        case "end":
                                m.vp.GotoBottom()
                                m.userScrolledUp = false
                                return m, nil
                        }
                }

                switch msg.String() {
                case "ctrl+c":
                        if m.state == stateRunning {
                                m.quit = true
                                return m, tea.Quit
                        }
                        m.quit = true
                        return m, tea.Quit

                case "ctrl+l":
                        m.vp.GotoBottom()
                        m.userScrolledUp = false
                        return m, nil

                case "enter":
                        if m.state == stateRunning {
                                return m, nil
                        }

                        input := strings.TrimSpace(m.input)
                        m.input = ""
                        m.cursor = 0

                        if strings.HasPrefix(input, "/") {
                                return m.handleCommand(input)
                        }

                        if input == "" {
                                return m, nil
                        }

                        m.history = append(m.history, input)
                        m.histIdx = len(m.history)

                        m.output = append(m.output, OutputLine{Type: "user", Content: input})
                        m.markDirty()
                        m.ensureContent()

                        m.state = stateRunning
                        m.streamText = ""
                        m.activeTool = ""
                        m.drainDone = false
                        m.streamCh = make(chan streamEvent, 256)
                        m.resultCh = make(chan agentResult, 1)
                        m.userScrolledUp = false
                        m.vp.GotoBottom()
                        return m, m.runAgent(input)

                case "up":
                        if m.histIdx > 0 {
                                m.histIdx--
                                m.input = m.history[m.histIdx]
                                m.cursor = len(m.input)
                        } else if m.histIdx == 0 {
                                m.input = m.history[0]
                                m.cursor = len(m.input)
                        }

                case "down":
                        if m.histIdx < len(m.history)-1 {
                                m.histIdx++
                                m.input = m.history[m.histIdx]
                                m.cursor = len(m.input)
                        } else {
                                m.histIdx = len(m.history)
                                m.input = ""
                                m.cursor = 0
                        }

                case "backspace":
                        if m.cursor > 0 && m.cursor <= len(m.input) {
                                m.input = m.input[:m.cursor-1] + m.input[m.cursor:]
                                m.cursor--
                        }

                case "ctrl+w":
                        if m.cursor > 0 {
                                i := m.cursor - 1
                                for i > 0 && m.input[i] == ' ' {
                                        i--
                                }
                                for i > 0 && m.input[i-1] != ' ' {
                                        i--
                                }
                                m.input = m.input[:i] + m.input[m.cursor:]
                                m.cursor = i
                        }

                case "ctrl+u":
                        m.input = ""
                        m.cursor = 0

                case "home":
                        m.cursor = 0

                case "end":
                        m.cursor = len(m.input)

                case "left":
                        if m.cursor > 0 {
                                m.cursor--
                        }

                case "right":
                        if m.cursor < len(m.input) {
                                m.cursor++
                        }

                default:
                        if len(msg.String()) == 1 {
                                if m.cursor < len(m.input) {
                                        m.input = m.input[:m.cursor] + msg.String() + m.input[m.cursor:]
                                } else {
                                        m.input += msg.String()
                                }
                                m.cursor++
                        }
                }

        case drainStreamMsg:
                for {
                        select {
                        case evt, ok := <-m.streamCh:
                                if !ok {
                                        m.drainDone = true
                                } else {
                                        m.handleStreamEvent(evt)
                                }
                        default:
                                goto checkDone
                        }
                }
        checkDone:
                if !m.drainDone {
                        select {
                        case result := <-m.resultCh:
                                m.drainDone = true
                                for evt := range m.streamCh {
                                        m.handleStreamEvent(evt)
                                }
                                m.flushStreamText()
                                m.state = stateIdle
                                if result.err != nil {
                                        m.err = result.err
                                        m.output = append(m.output, OutputLine{
                                                Type:    "error",
                                                Content: result.err.Error(),
                                        })
                                }
                                if len(m.agent.History()) > 0 {
                                        m.autoSaveSession()
                                }
                                m.ensureContent()
                                m.vp.GotoBottom()
                                m.userScrolledUp = false
                                return m, nil
                        default:
                        }
                }

                if !m.userScrolledUp {
                        m.rebuildViewportContent()
                        m.vp.GotoBottom()
                }

                if m.state == stateRunning {
                        cmds = append(cmds, drainStream())
                }
                return m, tea.Batch(cmds...)

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
                if len(m.agent.History()) > 0 {
                        m.autoSaveSession()
                }
                m.ensureContent()
                m.vp.GotoBottom()
                m.userScrolledUp = false
                return m, nil

        case spinnerTickMsg:
                m.spinner = (m.spinner + 1) % len(spinnerChars)
                if m.state == stateRunning {
                        if !m.userScrolledUp {
                                m.rebuildViewportContent()
                                m.vp.GotoBottom()
                        }
                        cmds = append(cmds, tickSpinner())
                }
                return m, tea.Batch(cmds...)

        case tea.MouseMsg:
                if m.userScrolledUp {
                        prevBottom := m.vp.AtBottom()
                        newVp, cmd := m.vp.Update(msg)
                        m.vp = newVp
                        if prevBottom && m.vp.AtBottom() {
                                // Was at bottom, still at bottom
                        } else if m.vp.AtBottom() {
                                m.userScrolledUp = false
                        }
                        if cmd != nil {
                                cmds = append(cmds, cmd)
                        }
                        return m, tea.Batch(cmds...)
                }
        }

        return m, tea.Batch(cmds...)
}

// View renders the model.
func (m replModel) View() string {
        if m.quit && m.err == nil {
                return ""
        }

        // Header
        header := titleStyle.Render(fmt.Sprintf("\u26A1 Cairn Code %s", m.version))
        if m.agent != nil {
                header += systemStyle.Render(fmt.Sprintf("  [%s / %s]", m.agent.ProviderName(), m.agent.Model()))
        }
        if m.sessionID != "" {
                shortID := m.sessionID
                if len(shortID) > 8 {
                        shortID = shortID[:8]
                }
                header += systemStyle.Render(fmt.Sprintf("  session: %s", shortID))
        }

        // Scroll hint
        var scrollHint string
        if m.userScrolledUp && m.state == stateIdle {
                scrollHint = "\n" + scrollHintStyle.Render("  \u2193 Ctrl+L to jump to bottom")
        }

        // Footer
        var footer strings.Builder

        m.mu.Lock()
        activeTool := m.activeTool
        streamingText := m.streamText
        m.mu.Unlock()

        if m.state == stateRunning {
                spin := spinnerStyle.Render(spinnerChars[m.spinner])
                if activeTool != "" {
                        footer.WriteString(activityStyle.Render(fmt.Sprintf(" %s  Running %s...", spin, toolNameStyle.Render(activeTool))))
                } else if streamingText == "" {
                        footer.WriteString(activityStyle.Render(fmt.Sprintf(" %s  Thinking...", spin)))
                }
                footer.WriteString("\n")
        }

        if m.totalUsage.InputTokens > 0 {
                u := fmt.Sprintf("Tokens: %d in, %d out", m.totalUsage.InputTokens, m.totalUsage.OutputTokens)
                if m.totalUsage.CacheRead > 0 {
                        u += fmt.Sprintf(" (cache: %d read, %d create)", m.totalUsage.CacheRead, m.totalUsage.CacheCreate)
                }
                footer.WriteString(usageStyle.Render(u))
                footer.WriteString("\n")
        }

        if !m.quit {
                footer.WriteString(promptStyle.Render("\u27D8 "))
                footer.WriteString(m.input)
                footer.WriteString("\n")
        }

        // During streaming, rebuild with live text + spinner
        if m.state == stateRunning {
                m.rebuildViewportContent()
                if !m.userScrolledUp {
                        m.vp.GotoBottom()
                }
        }

        return header + "\n\n" + m.vp.View() + scrollHint + "\n" + footer.String()
}

// handleStreamEvent processes a single stream event.
func (m *replModel) handleStreamEvent(evt streamEvent) {
        switch evt.typ {
        case "text":
                m.mu.Lock()
                m.streamText += evt.text
                m.mu.Unlock()

        case "tool_use":
                m.flushStreamText()
                m.mu.Lock()
                m.activeTool = evt.toolName
                m.mu.Unlock()
                summary := formatToolSummary(evt.toolName, evt.toolInput)
                m.output = append(m.output, OutputLine{Type: "tool_use", ToolName: evt.toolName, Content: summary})
                m.markDirty()
                m.ensureContent()

        case "tool_result":
                m.mu.Lock()
                m.activeTool = ""
                m.mu.Unlock()
                m.output = append(m.output, OutputLine{Type: "tool_result", ToolName: evt.toolName, Content: evt.toolOutput, Duration: evt.duration})
                m.markDirty()
                m.ensureContent()

        case "error":
                m.output = append(m.output, OutputLine{Type: "error", Content: evt.text})
                m.markDirty()
                m.ensureContent()

        case "turn_end":
                m.flushStreamText()
                m.totalUsage.InputTokens += evt.usage.InputTokens
                m.totalUsage.OutputTokens += evt.usage.OutputTokens
                m.totalUsage.CacheRead += evt.usage.CacheRead
                m.totalUsage.CacheCreate += evt.usage.CacheCreate
                m.markDirty()
                m.ensureContent()
        }
}

// flushStreamText flushes accumulated streaming text to output.
func (m *replModel) flushStreamText() {
        m.mu.Lock()
        text := m.streamText
        m.streamText = ""
        m.mu.Unlock()

        text = strings.TrimRight(text, " \t\r\n")
        if text == "" {
                return
        }
        m.output = append(m.output, OutputLine{Type: "text", Content: text})
        m.markDirty()
}

// renderOutputLine renders a single output line.
func (m *replModel) renderOutputLine(line OutputLine) string {
        switch line.Type {
        case "user":
                return userStyle.Render("\u27D8 " + line.Content) + "\n\n"

        case "text":
                rendered := line.Content
                if m.renderer != nil {
                        if md, err := m.renderer.Render(line.Content); err == nil {
                                rendered = md
                        }
                }
                return rendered + "\n"

        case "tool_use":
                if line.Content != "" {
                        return toolNameStyle.Render(fmt.Sprintf("\u25B8 %s  %s", line.ToolName, line.Content)) + "\n"
                }
                return toolNameStyle.Render(fmt.Sprintf("\u25B8 %s", line.ToolName)) + "\n"

        case "tool_result":
                var b strings.Builder
                isError := strings.HasPrefix(line.Content, "Error:")
                if isError {
                        b.WriteString(toolErrorStyle.Render(fmt.Sprintf("  \u2717 %s", line.ToolName)))
                } else {
                        b.WriteString(toolSuccessStyle.Render(fmt.Sprintf("  \u2713 %s", line.ToolName)))
                }
                if line.Duration > 0 {
                        b.WriteString(usageStyle.Render(fmt.Sprintf(" (%.1fs)", line.Duration.Seconds())))
                }
                b.WriteString("\n")

                content := strings.TrimSpace(line.Content)
                if content != "" {
                        if len(content) > 2000 {
                                content = content[:2000] + "\n  ... [output truncated]"
                        }
                        b.WriteString(toolResultStyle.Render(indent(content, "    ")))
                        b.WriteString("\n")
                }
                return b.String()

        case "error":
                return errorStyle.Render("\u2717 " + line.Content) + "\n\n"

        case "system":
                return systemStyle.Render(line.Content) + "\n"

        default:
                return line.Content + "\n"
        }
}

// handleCommand processes slash commands.
func (m replModel) handleCommand(cmd string) (tea.Model, tea.Cmd) {
        parts := strings.Fields(cmd)
        if len(parts) == 0 {
                return m, nil
        }

        switch parts[0] {
        case "/help", "/?":
                m.output = append(m.output, OutputLine{Type: "system", Content: `Available commands:
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
  /undo              Remove the last exchange (user prompt + agent response)
  /quit, /exit       Exit the application

Keyboard shortcuts:
  Ctrl+C             Quit
  Ctrl+L             Scroll to bottom
  Ctrl+W             Delete word backward
  Ctrl+U             Clear input
  Up/Down            History navigation
  Home/End           Move cursor to start/end
  PgUp/PgDn          Scroll output (when scrolled up)
  Mouse wheel        Scroll output`})
                m.markDirty()
                m.ensureContent()
                m.vp.GotoBottom()
                return m, nil

        case "/clear":
                m.agent.Reset()
                m.totalUsage = llm.Usage{}
                m.sessionID = ""
                m.output = nil
                m.vp.SetContent("")
                m.cachedContent = ""
                m.userScrolledUp = false
                m.output = append(m.output, OutputLine{Type: "system", Content: "Conversation cleared."})
                m.markDirty()
                m.ensureContent()
                m.vp.GotoBottom()
                return m, nil

        case "/undo":
                m.undoLastExchange()
                m.markDirty()
                m.ensureContent()
                m.vp.GotoBottom()
                return m, nil

        case "/compact":
                m.state = stateRunning
                return m, m.runCompact()

        case "/model":
                if len(parts) > 1 {
                        newModel := strings.Join(parts[1:], " ")
                        m.agent.SetModel(newModel)
                        m.output = append(m.output, OutputLine{Type: "system", Content: fmt.Sprintf("Model set to: %s", newModel)})
                } else {
                        m.output = append(m.output, OutputLine{Type: "system", Content: fmt.Sprintf("Current model: %s (provider: %s)", m.agent.Model(), m.agent.ProviderName())})
                }
                m.markDirty()
                m.ensureContent()
                m.vp.GotoBottom()
                return m, nil

        case "/cost":
                cost := fmt.Sprintf("Token usage:\n  Input:  %d\n  Output: %d", m.totalUsage.InputTokens, m.totalUsage.OutputTokens)
                if m.totalUsage.CacheRead > 0 || m.totalUsage.CacheCreate > 0 {
                        cost += fmt.Sprintf("\n  Cache read:  %d\n  Cache create: %d", m.totalUsage.CacheRead, m.totalUsage.CacheCreate)
                }
                m.output = append(m.output, OutputLine{Type: "system", Content: cost})
                m.markDirty()
                m.ensureContent()
                m.vp.GotoBottom()
                return m, nil

        case "/provider":
                m.output = append(m.output, OutputLine{Type: "system", Content: fmt.Sprintf("Provider: %s", m.agent.ProviderName())})
                m.markDirty()
                m.ensureContent()
                m.vp.GotoBottom()
                return m, nil

        case "/save":
                return m, m.saveCurrentSession()

        case "/resume":
                resumeID := ""
                if len(parts) > 1 {
                        resumeID = parts[1]
                }
                m.state = stateRunning
                return m, m.resumeSession(resumeID)

        case "/sessions":
                m.state = stateRunning
                return m, m.listSessions()

        case "/tools":
                m.output = append(m.output, OutputLine{Type: "system", Content: "Available tools: file_read, file_write, file_edit, bash, glob, grep, todo_write, web_search, web_fetch"})
                m.markDirty()
                m.ensureContent()
                m.vp.GotoBottom()
                return m, nil

        case "/quit", "/exit", "/q":
                m.quit = true
                return m, tea.Quit

        default:
                m.output = append(m.output, OutputLine{Type: "error", Content: fmt.Sprintf("Unknown command: %s (type /help for available commands)", parts[0])})
                m.markDirty()
                m.ensureContent()
                m.vp.GotoBottom()
                return m, nil
        }
}

// undoLastExchange removes the last user message and all subsequent messages.
func (m *replModel) undoLastExchange() {
        msgs := m.agent.History()
        if len(msgs) == 0 {
                m.output = append(m.output, OutputLine{Type: "system", Content: "Nothing to undo."})
                return
        }

        lastUserIdx := -1
        for i := len(msgs) - 1; i >= 0; i-- {
                if msgs[i].Role == llm.RoleUser {
                        lastUserIdx = i
                        break
                }
        }
        if lastUserIdx < 0 {
                m.output = append(m.output, OutputLine{Type: "system", Content: "Nothing to undo."})
                return
        }

        m.agent.SetMessages(msgs[:lastUserIdx])

        cutIdx := -1
        for i := len(m.output) - 1; i >= 0; i-- {
                if m.output[i].Type == "user" {
                        cutIdx = i
                        break
                }
        }
        if cutIdx >= 0 {
                m.output = m.output[:cutIdx]
        }

        m.output = append(m.output, OutputLine{Type: "system", Content: "Last exchange undone."})
}

// runAgent runs the agent in a goroutine and streams events.
func (m replModel) runAgent(input string) tea.Cmd {
        streamCh := m.streamCh
        resultCh := m.resultCh

        return func() tea.Msg {
                defer close(streamCh)
                var agentErr error

                cb := agent.Callbacks{
                        OnText: func(text string) {
                                streamCh <- streamEvent{typ: "text", text: text}
                        },
                        OnToolUse: func(name string, input any) {
                                streamCh <- streamEvent{typ: "tool_use", toolName: name, toolInput: input}
                        },
                        OnToolResult: func(name string, output string, duration time.Duration) {
                                streamCh <- streamEvent{typ: "tool_result", toolName: name, toolOutput: output, duration: duration}
                        },
                        OnTurnEnd: func(turn int, usage llm.Usage) {
                                streamCh <- streamEvent{typ: "turn_end", usage: usage}
                        },
                        OnError: func(err error) {
                                streamCh <- streamEvent{typ: "error", text: err.Error()}
                        },
                        OnPermission: func(tool string, input any) bool {
                                return true
                        },
                }

                a := m.agent
                a.SetCallbacks(cb)
                agentErr = a.Run(context.Background(), input)

                resultCh <- agentResult{err: agentErr}
                return nil
        }
}

func drainStream() tea.Cmd {
        return tea.Tick(time.Millisecond*16, func(t time.Time) tea.Msg {
                return drainStreamMsg{}
        })
}

func (m replModel) runCompact() tea.Cmd {
        return func() tea.Msg {
                a := m.agent
                a.SetCallbacks(agent.Callbacks{OnError: func(err error) {}})
                if err := a.Compact(context.Background()); err != nil {
                        return agentCompleteMsg{
                                output: []OutputLine{{Type: "error", Content: fmt.Sprintf("Compaction failed: %v", err)}},
                                usage:  llm.Usage{},
                                err:    err,
                        }
                }
                return agentCompleteMsg{
                        output: []OutputLine{{Type: "system", Content: "Conversation compacted successfully."}},
                        usage:  llm.Usage{},
                }
        }
}

func (m replModel) saveCurrentSession() tea.Cmd {
        return func() tea.Msg {
                history := m.agent.History()
                if len(history) == 0 {
                        return agentCompleteMsg{output: []OutputLine{{Type: "system", Content: "Nothing to save."}}}
                }
                id := session.NewSessionID()
                sess := session.FromMessages(id, history, m.agent.Model(), m.agent.ProviderName(), m.totalUsage.InputTokens, m.totalUsage.OutputTokens)
                if err := session.SaveSession(m.sessionDir, sess); err != nil {
                        return agentCompleteMsg{output: []OutputLine{{Type: "error", Content: fmt.Sprintf("Failed to save session: %v", err)}}}
                }
                m.sessionID = id
                return agentCompleteMsg{output: []OutputLine{{Type: "system", Content: fmt.Sprintf("Session saved: %s", id)}}}
        }
}

func (m replModel) resumeSession(id string) tea.Cmd {
        return func() tea.Msg {
                sessDir := m.sessionDir
                if id == "" {
                        sessions, err := session.ListSessions(sessDir)
                        if err != nil {
                                return agentCompleteMsg{output: []OutputLine{{Type: "error", Content: fmt.Sprintf("Failed to list sessions: %v", err)}}}
                        }
                        if len(sessions) == 0 {
                                return agentCompleteMsg{output: []OutputLine{{Type: "system", Content: "No saved sessions found."}}}
                        }
                        id = sessions[0].ID
                }
                sess, err := session.LoadSession(sessDir, id)
                if err != nil {
                        return agentCompleteMsg{output: []OutputLine{{Type: "error", Content: fmt.Sprintf("Failed to load session %s: %v", id, err)}}}
                }
                if sess.Model != "" {
                        m.agent.SetModel(sess.Model)
                }
                m.agent.SetMessages(sess.ToMessages())
                m.totalUsage = llm.Usage{InputTokens: sess.TokensIn, OutputTokens: sess.TokensOut}
                m.sessionID = sess.ID
                return agentCompleteMsg{output: []OutputLine{{Type: "system", Content: fmt.Sprintf("Resumed session %s (model: %s, messages: %d)", sess.ID, sess.Model, len(sess.Messages))}}}
        }
}

func (m replModel) listSessions() tea.Cmd {
        return func() tea.Msg {
                sessions, err := session.ListSessions(m.sessionDir)
                if err != nil {
                        return agentCompleteMsg{output: []OutputLine{{Type: "error", Content: fmt.Sprintf("Failed to list sessions: %v", err)}}}
                }
                if len(sessions) == 0 {
                        return agentCompleteMsg{output: []OutputLine{{Type: "system", Content: "No saved sessions found."}}}
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
                return agentCompleteMsg{output: []OutputLine{{Type: "system", Content: buf.String()}}}
        }
}

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
        sess := session.FromMessages(id, history, m.agent.Model(), m.agent.ProviderName(), m.totalUsage.InputTokens, m.totalUsage.OutputTokens)
        if prev, err := session.LoadSession(m.sessionDir, id); err == nil {
                sess.CreatedAt = prev.CreatedAt
        }
        _ = session.SaveSession(m.sessionDir, sess)
}

// Message types

type agentResult struct{ err error }
type drainStreamMsg struct{}
type spinnerTickMsg time.Time

type agentCompleteMsg struct {
        output []OutputLine
        usage  llm.Usage
        err    error
}

func tickSpinner() tea.Cmd {
        return tea.Tick(time.Millisecond*80, func(t time.Time) tea.Msg {
                return spinnerTickMsg(t)
        })
}

func formatToolSummary(name string, input any) string {
        switch name {
        case "file_read":
                if m, ok := input.(map[string]any); ok {
                        if p, ok := m["path"].(string); ok {
                                return fmt.Sprintf("Read %s", p)
                        }
                }
        case "file_write":
                if m, ok := input.(map[string]any); ok {
                        if p, ok := m["path"].(string); ok {
                                return fmt.Sprintf("Write %s", p)
                        }
                }
        case "file_edit":
                if m, ok := input.(map[string]any); ok {
                        if p, ok := m["path"].(string); ok {
                                return fmt.Sprintf("Edit %s", p)
                        }
                }
        case "bash":
                if m, ok := input.(map[string]any); ok {
                        if cmd, ok := m["command"].(string); ok {
                                if len(cmd) > 80 {
                                        cmd = cmd[:80] + "..."
                                }
                                return fmt.Sprintf("$ %s", cmd)
                        }
                }
        case "grep":
                if m, ok := input.(map[string]any); ok {
                        if p, ok := m["pattern"].(string); ok {
                                return fmt.Sprintf("Grep: %s", p)
                        }
                }
        case "glob":
                if m, ok := input.(map[string]any); ok {
                        if p, ok := m["pattern"].(string); ok {
                                return fmt.Sprintf("Glob: %s", p)
                        }
                }
        case "web_search":
                if m, ok := input.(map[string]any); ok {
                        if q, ok := m["query"].(string); ok {
                                if len(q) > 60 {
                                        q = q[:60] + "..."
                                }
                                return fmt.Sprintf("Search: %s", q)
                        }
                }
        case "web_fetch":
                if m, ok := input.(map[string]any); ok {
                        if u, ok := m["url"].(string); ok {
                                if len(u) > 60 {
                                        u = u[:60] + "..."
                                }
                                return fmt.Sprintf("Fetch: %s", u)
                        }
                }
        }
        return name
}

func indent(text, prefix string) string {
        lines := strings.Split(text, "\n")
        for i, line := range lines {
                lines[i] = prefix + line
        }
        return strings.Join(lines, "\n")
}
