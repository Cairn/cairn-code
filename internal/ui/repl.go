package ui

import (
        "context"
        "encoding/json"
        "fmt"
        "html"
        "math/rand/v2"
        "os/exec"
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
        currentVerb string // picked once per turn, stays fixed (Claude Code style)
        streamingText string // text accumulating during streaming (rendered raw, no glamour yet)
        streamChunkCh  chan string          // channel for receiving streaming chunks from agent goroutine
        streamResultCh chan agentCompleteMsg // receives final agent result when goroutine finishes
        cursorBlink bool
        sessionDir string
        sessionID  string // current session ID for auto-save
        lastCtrlC  time.Time
        showQuit   bool   // whether quit confirmation is showing
        quitChoice int    // 0 = yes, 1 = no
        cmdSelect  int    // selected index in command autocomplete
        workDir    string
        version    string
        // Session picker
        showSessionPicker bool              // whether session picker is showing
        pickerSessions    []session.Session // sessions to pick from
        pickerSelect      int              // selected index in picker
        pickerScroll      int              // scroll offset for picker
}

var (
        // Styles — Claude Code inspired color palette
        promptStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("215")) // Claude orange

        userStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("215")) // Claude orange for user messages

        toolNameStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("252")) // bright white for tool names

        toolResultStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245")) // dim

        errorStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("196")) // red

        textStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("252")) // bright white for streaming text

        systemStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245")) // dim

        usageStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245")) // dim

        titleStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("215")) // Claude orange

        brandStyle = lipgloss.NewStyle().
                        Bold(true).
                        Foreground(lipgloss.Color("215")) // Claude orange

        dimBorderStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245")) // dim border

        labelStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("245")) // dim labels

        successStyle = lipgloss.NewStyle().
                        Foreground(lipgloss.Color("78")) // green ●

        // Spinner: smooth braille wave pattern
        spinnerChars = []string{"⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"}

        // Playful spinner verbs (Claude Code style) — picked once per turn at random
        spinnerVerbs = []string{
                // Cognitive
                "Thinking", "Clauding", "Cogitating", "Contemplating", "Deliberating",
                "Mulling", "Pondering", "Ruminating", "Musing", "Percolating",
                "Reasoning", "Analyzing", "Reflecting", "Processing", "Computing",
                "Brainstorming", "Architecting", "Synthesizing", "Constructing", "Evaluating",
                // Action
                "Crafting", "Building", "Forging", "Shaping", "Wiring",
                "Assembling", "Implementing", "Orchestrating", "Refining", "Polishing",
                "Exploring", "Investigating", "Tracing", "Navigating", "Searching",
                "Generating", "Resolving", "Optimizing", "Compiling", "Debugging",
                // Whimsical
                "Claude-hopping", "Baking", "Brewing", "Simmering", "Sautéing",
                "Caramelizing", "Garnishing", "Kneading", "Zesting", "Fermenting",
                "Beboppin'", "Moonwalking", "Razzle-dazzling", "Gitifying", "Wibbling",
                "Reticulating", "Quantumizing", "Crunching", "Churning", "MacGyvering",
        }

        toolDescriptions = map[string]string{
                "bash":               "shell command",
                "file_read":          "read file",
                "file_write":         "write file",
                "file_edit":          "edit file",
                "git":                "git operation",
                "glob":               "find files",
                "grep":               "search files",
                "todo_write":         "update todos",
                "memory":             "memory operation",
                "web_search":         "web search",
                "web_fetch":          "fetch URL",
                "create_pull_request": "create PR",
                "github_issue":       "GitHub issue",
        }
)

var (
        // Command definitions for autocomplete
        commands = []cmdDef{
                {"/clear", "Clear conversation history"},
                {"/model", "Show or change model"},
                {"/compact", "Compact conversation"},
                {"/cost", "Show token usage"},
                {"/provider", "Show current provider"},
                {"/save", "Save session"},
                {"/resume", "Resume a session"},
                {"/sessions", "List saved sessions"},
                {"/tools", "List available tools"},
                {"/exit", "Exit application"},
        }
)

type cmdDef struct {
        name string
        desc string
}

// filteredCommands returns commands matching the given prefix.
func filteredCommands(prefix string) []cmdDef {
        prefix = strings.ToLower(prefix)
        var matches []cmdDef
        for _, c := range commands {
                if strings.HasPrefix(strings.ToLower(c.name), prefix) {
                        matches = append(matches, c)
                }
        }
        return matches
}

// showAutocomplete returns true if we should show the command palette.
func (m *replModel) showAutocomplete() bool {
        return strings.HasPrefix(m.input, "/") && !m.showQuit && !m.showSessionPicker
}

// NewREPL creates a new REPL model.
func NewREPL(a *agent.Agent, sessionDir, workDir, version string) *replModel {
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
                workDir:    workDir,
                version:    version,
        }
}

// Init initializes the model.
func (m *replModel) Init() tea.Cmd {
        return tea.Batch(tickSpinner(), tickCursorBlink())
}

// Update handles messages.
func (m *replModel) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
        switch msg := msg.(type) {
        case tea.WindowSizeMsg:
                m.width = msg.Width
                m.height = msg.Height
                return m, nil

        case tea.KeyMsg:
                switch msg.String() {
                case "ctrl+c":
                        if m.showSessionPicker {
                                m.showSessionPicker = false
                                m.pickerSessions = nil
                                m.pickerSelect = 0
                                m.pickerScroll = 0
                                return m, nil
                        }
                        if m.showQuit {
                                // Already showing quit dialog — dismiss it
                                m.showQuit = false
                                return m, nil
                        }
                        if m.state == stateRunning {
                                // Agent is running — quit immediately
                                m.quit = true
                                return m, tea.Quit
                        }
                        // If pressed within 500ms of last Ctrl+C, show quit dialog
                        if time.Since(m.lastCtrlC) < 500*time.Millisecond {
                                m.showQuit = true
                                m.quitChoice = 1 // default to No
                                return m, nil
                        }
                        // Single press — clear input
                        m.lastCtrlC = time.Now()
                        m.input = ""
                        m.cursor = 0
                        return m, nil

                case "enter":
                        if m.showSessionPicker {
                                if m.pickerSelect >= 0 && m.pickerSelect < len(m.pickerSessions) {
                                        selected := m.pickerSessions[m.pickerSelect]
                                        m.showSessionPicker = false
                                        m.pickerSessions = nil
                                        m.pickerSelect = 0
                                        m.pickerScroll = 0
                                        m.state = stateRunning
                                        m.currentVerb = pickSpinnerVerb()
                                        return m, tea.Batch(m.resumeSession(selected.ID), tickSpinner())
                                }
                                m.showSessionPicker = false
                                m.pickerSessions = nil
                                return m, nil
                        }
                        if m.showQuit {
                                if m.quitChoice == 0 {
                                        m.quit = true
                                        return m, tea.Quit
                                }
                                m.showQuit = false
                                return m, nil
                        }
                        // Autocomplete: select command and execute
                        if m.showAutocomplete() {
                                matches := filteredCommands(m.input)
                                if m.cmdSelect < len(matches) {
                                        m.input = matches[m.cmdSelect].name + " "
                                        m.cursor = len(m.input)
                                        m.cmdSelect = 0
                                        return m, nil
                                }
                        }
                        if m.state == stateRunning {
                                return m, nil
                        }

                        input := strings.TrimSpace(m.input)
                        m.input = ""
                        m.cursor = 0
                        m.cmdSelect = 0

                        // Handle commands
                        if strings.HasPrefix(input, "/") {
                                return m.handleCommand(input)
                        }

                        if input == "" {
                                return m, nil
                        }

                        // Bash mode: ! prefix runs shell command directly
                        if strings.HasPrefix(input, "!") {
                                bashCmd := strings.TrimPrefix(input, "!")
                                bashCmd = strings.TrimSpace(bashCmd)
                                if bashCmd != "" {
                                        m.history = append(m.history, input)
                                        m.histIdx = len(m.history)
                                        m.output = append(m.output, OutputLine{
                                                Type:    "user",
                                                Content: input,
                                        })
                                        m.state = stateRunning
                                        m.currentVerb = pickSpinnerVerb()
                                        return m, tea.Batch(m.runBashCommand(bashCmd), tickSpinner())
                                }
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
                        m.currentVerb = pickSpinnerVerb()
                        return m, tea.Batch(m.runAgent(input), tickSpinner())

                case "up":
                        if m.showSessionPicker {
                                if m.pickerSelect > 0 {
                                        m.pickerSelect--
                                        if m.pickerSelect < m.pickerScroll {
                                                m.pickerScroll = m.pickerSelect
                                        }
                                }
                                return m, nil
                        }
                        if m.showAutocomplete() {
                                matches := filteredCommands(m.input)
                                if len(matches) > 0 {
                                        m.cmdSelect--
                                        if m.cmdSelect < 0 {
                                                m.cmdSelect = len(matches) - 1
                                        }
                                }
                                return m, nil
                        }
                        if len(m.history) > 0 {
                                if m.histIdx > 0 {
                                        m.histIdx--
                                }
                                m.input = m.history[m.histIdx]
                        }

                case "down":
                        if m.showSessionPicker {
                                if m.pickerSelect < len(m.pickerSessions)-1 {
                                        m.pickerSelect++
                                        // Visible height: dialog height minus borders minus title minus footer
                                        visibleHeight := m.pickerVisibleHeight()
                                        if m.pickerSelect >= m.pickerScroll+visibleHeight {
                                                m.pickerScroll = m.pickerSelect - visibleHeight + 1
                                        }
                                }
                                return m, nil
                        }
                        if m.showAutocomplete() {
                                matches := filteredCommands(m.input)
                                if len(matches) > 0 {
                                        m.cmdSelect++
                                        if m.cmdSelect >= len(matches) {
                                                m.cmdSelect = 0
                                        }
                                }
                                return m, nil
                        }
                        if m.histIdx < len(m.history)-1 {
                                m.histIdx++
                                m.input = m.history[m.histIdx]
                        } else {
                                m.histIdx = len(m.history)
                                m.input = ""
                        }

                case "tab":
                        if m.showAutocomplete() {
                                matches := filteredCommands(m.input)
                                if m.cmdSelect < len(matches) {
                                        m.input = matches[m.cmdSelect].name + " "
                                        m.cursor = len(m.input)
                                        m.cmdSelect = 0
                                }
                                return m, nil
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
                        if m.showQuit {
                                m.quitChoice = 0 // Yes
                                return m, nil
                        }
                        if m.cursor > 0 {
                                m.cursor--
                        }

                case "right":
                        if m.showQuit {
                                m.quitChoice = 1 // No
                                return m, nil
                        }
                        if m.cursor < len(m.input) {
                                m.cursor++
                        }

                // Quit dialog shortcuts
                case "y", "Y":
                        if m.showQuit {
                                m.quitChoice = 0
                                m.quit = true
                                return m, tea.Quit
                        }
                        // Not in quit dialog — insert as normal character
                        if m.cursor < len(m.input) {
                                m.input = m.input[:m.cursor] + msg.String() + m.input[m.cursor:]
                        } else {
                                m.input += msg.String()
                        }
                        m.cursor++
                        m.cmdSelect = 0
                case "n", "N":
                        if m.showQuit {
                                m.showQuit = false
                                return m, nil
                        }
                        // Dismiss autocomplete if showing
                        if m.showAutocomplete() {
                                m.input = ""
                                m.cursor = 0
                                m.cmdSelect = 0
                                return m, nil
                        }
                        // Not in quit dialog or autocomplete — insert as normal character
                        if m.cursor < len(m.input) {
                                m.input = m.input[:m.cursor] + msg.String() + m.input[m.cursor:]
                        } else {
                                m.input += msg.String()
                        }
                        m.cursor++
                        m.cmdSelect = 0
                case "esc":
                        if m.showSessionPicker {
                                m.showSessionPicker = false
                                m.pickerSessions = nil
                                m.pickerSelect = 0
                                m.pickerScroll = 0
                                return m, nil
                        }
                        if m.showQuit {
                                m.showQuit = false
                                return m, nil
                        }
                        // Dismiss autocomplete
                        if m.showAutocomplete() {
                                m.input = ""
                                m.cursor = 0
                                m.cmdSelect = 0
                                return m, nil
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
                                m.cmdSelect = 0
                        }
                }

        case agentCompleteMsg:
                m.state = stateIdle
                m.output = append(m.output, msg.output...)
                m.totalUsage.InputTokens += msg.usage.InputTokens
                m.totalUsage.OutputTokens += msg.usage.OutputTokens
                m.totalUsage.CacheRead += msg.usage.CacheRead
                m.totalUsage.CacheCreate += msg.usage.CacheCreate
                m.streamingText = "" // clear streaming buffer
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

        case drainStreamMsg:
                // Non-blocking drain: read all pending chunks from channel
                if m.streamChunkCh != nil {
                        for {
                                select {
                                case chunk, ok := <-m.streamChunkCh:
                                        if !ok {
                                                // Channel closed — agent goroutine finished
                                                m.streamChunkCh = nil
                                                // Read the final result and process inline
                                                if m.streamResultCh != nil {
                                                        result := <-m.streamResultCh
                                                        m.streamResultCh = nil
                                                        // Process the completion inline
                                                        m.state = stateIdle
                                                        m.output = append(m.output, result.output...)
                                                        m.totalUsage.InputTokens += result.usage.InputTokens
                                                        m.totalUsage.OutputTokens += result.usage.OutputTokens
                                                        m.totalUsage.CacheRead += result.usage.CacheRead
                                                        m.totalUsage.CacheCreate += result.usage.CacheCreate
                                                        m.streamingText = ""
                                                        if result.err != nil {
                                                                m.err = result.err
                                                        }
                                                        if len(m.agent.History()) > 0 {
                                                                m.autoSaveSession()
                                                        }
                                                }
                                                break
                                        }
                                        m.streamingText += chunk
                                        // Check for stream clear sentinel (from OnText commit)
                                        if strings.Contains(m.streamingText, "\x00STREAM_CLEAR\x00") {
                                                m.streamingText = ""
                                        }
                                default:
                                        // No more chunks available right now
                                        break
                                }
                        }
                }
                // Keep polling while running
                if m.state == stateRunning {
                        return m, drainStreamTicks()
                }
                return m, nil

        case cursorBlinkMsg:
                m.cursorBlink = !m.cursorBlink
                return m, tickCursorBlink()

        case sessionsLoadedMsg:
                m.state = stateIdle
                if msg.err != nil {
                        m.output = append(m.output, OutputLine{
                                Type:    "error",
                                Content: fmt.Sprintf("Failed to list sessions: %v", msg.err),
                        })
                        return m, nil
                }
                if len(msg.sessions) == 0 {
                        m.output = append(m.output, OutputLine{
                                Type:    "system",
                                Content: "No saved sessions found.",
                        })
                        return m, nil
                }
                // Show the picker
                m.showSessionPicker = true
                m.pickerSessions = msg.sessions
                m.pickerSelect = 0
                m.pickerScroll = 0
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

        // Welcome banner (shown once at start, before any output)
        if len(m.output) == 0 && m.state == stateIdle {
                content.WriteString(m.renderBanner())
                content.WriteString("\n")
        }

        // Output
        for _, line := range m.output {
                content.WriteString(m.renderOutputLine(line))
        }

        // Streaming text (rendered live as it arrives)
        if m.streamingText != "" {
                // Split into complete lines + partial last line
                // Render complete lines through glamour, leave partial line raw
                lines := strings.Split(m.streamingText, "\n")
                for i, line := range lines {
                        if i == len(lines)-1 && line == "" {
                                // Trailing newline from split — skip
                                continue
                        }
                        if i == len(lines)-1 {
                                // Last (potentially incomplete) line — render raw
                                content.WriteString(textStyle.Render(line))
                        } else {
                                // Complete line — render through glamour
                                content.WriteString(m.renderStreamingLine(line))
                                content.WriteString("\n")
                        }
                }
                content.WriteString("\n")
        }

        // Spinner if running (shows below streaming text)
        if m.state == stateRunning {
                verb := m.currentVerb
                if verb == "" {
                        verb = spinnerVerbs[0]
                }
                content.WriteString(fmt.Sprintf("%s %s…", spinnerChars[m.spinner], verb))
        }

        // Token usage
        if m.totalUsage.InputTokens > 0 {
                content.WriteString(usageStyle.Render(fmt.Sprintf(
                        "\nTokens: %d in, %d out",
                        m.totalUsage.InputTokens,
                        m.totalUsage.OutputTokens,
                )))
                content.WriteString("\n")
        }

        // Input prompt with cursor and autocomplete
        if !m.quit {
                content.WriteString("\n")

                // Command autocomplete dropdown
                if m.showAutocomplete() {
                        matches := filteredCommands(m.input)
                        if len(matches) > 0 {
                                if m.cmdSelect >= len(matches) {
                                        m.cmdSelect = 0
                                }
                                dimStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("245"))
                                selectedStyle := promptStyle.Bold(true)
                                for i, cmd := range matches {
                                        if i == m.cmdSelect {
                                                content.WriteString(selectedStyle.Render("  ▸ " + cmd.name))
                                        } else {
                                                content.WriteString(dimStyle.Render("    " + cmd.name))
                                        }
                                        // Pad description
                                        desc := "  " + cmd.desc
                                        // Truncate description to fit
                                        maxDesc := 30
                                        if len(desc) > maxDesc {
                                                desc = desc[:maxDesc-3] + "..."
                                        }
                                        content.WriteString(dimStyle.Render(desc))
                                        content.WriteString("\n")
                                }
                        }
                }

                content.WriteString(promptStyle.Render("❯ "))
                // Render input with cursor indicator
                before := m.input[:m.cursor]
                after := m.input[m.cursor:]
                content.WriteString(before)
                if m.cursorBlink && m.state != stateRunning {
                        content.WriteString(promptStyle.Render("▋"))
                }
                content.WriteString(after)
        }

        // Session picker overlay
        if m.showSessionPicker {
                return m.renderSessionPicker()
        }

        // Quit confirmation overlay
        if m.showQuit {
                dialog := m.renderQuitDialog()
                return dialog
        }

        return content.String()
}

// pickerVisibleHeight returns the number of session rows that fit in the picker dialog.
func (m *replModel) pickerVisibleHeight() int {
        // Reserve: top border(1) + title(1) + separator(1) + footer hint(1) + bottom border(1) = 5
        // Also leave some margin at top and bottom of terminal
        maxDialogHeight := m.height - 6
        if maxDialogHeight < 5 {
                maxDialogHeight = 5
        }
        visible := maxDialogHeight - 5 // subtract borders/title/footer
        if visible < 1 {
                visible = 1
        }
        if visible > len(m.pickerSessions) {
                visible = len(m.pickerSessions)
        }
        return visible
}

// renderSessionPicker renders a centered session picker overlay.
func (m *replModel) renderSessionPicker() string {
        dialogWidth := 70
        numSessions := len(m.pickerSessions)
        visibleHeight := m.pickerVisibleHeight()
        dialogHeight := 5 + visibleHeight // border + title + separator + items + footer + border

        // Calculate center position
        vertPad := (m.height - dialogHeight) / 2
        if vertPad < 1 {
                vertPad = 1
        }
        horizPad := (m.width - dialogWidth) / 2
        if horizPad < 2 {
                horizPad = 2
        }

        var b strings.Builder

        // Vertical padding to center
        for i := 0; i < vertPad; i++ {
                b.WriteString("\n")
        }

        // Horizontal padding
        horizSpace := strings.Repeat(" ", horizPad)

        // Styles
        borderStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("63"))
        yellowStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("221")).Bold(true)
        dimStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("245"))
        selectedBg := lipgloss.NewStyle().Background(lipgloss.Color("63")).Foreground(lipgloss.Color("230")).Bold(true)
        metaStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("245"))
        summaryStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("252"))

        // Top border
        b.WriteString(horizSpace)
        b.WriteString(borderStyle.Render("┌" + strings.Repeat("─", dialogWidth-2) + "┐"))
        b.WriteString("\n")

        // Title line
        b.WriteString(horizSpace)
        titleText := "  ⚡ Resume Session"
        titlePad := dialogWidth - 2 - len(titleText)
        if titlePad < 0 {
                titlePad = 0
        }
        line := fmt.Sprintf("%s%s%s", borderStyle.Render("│"), yellowStyle.Render(titleText+strings.Repeat(" ", titlePad)), borderStyle.Render("│"))
        b.WriteString(line)
        b.WriteString("\n")

        // Separator line
        b.WriteString(horizSpace)
        b.WriteString(borderStyle.Render("├" + strings.Repeat("─", dialogWidth-2) + "┤"))
        b.WriteString("\n")

        // Session entries (scrollable)
        endIdx := m.pickerScroll + visibleHeight
        if endIdx > numSessions {
                endIdx = numSessions
        }

        for i := m.pickerScroll; i < endIdx; i++ {
                s := m.pickerSessions[i]
                b.WriteString(horizSpace)
                b.WriteString(borderStyle.Render("│"))

                // Build the session entry line
                shortID := s.ID[:8]
                summary := s.Summary
                if summary == "" {
                        // Generate a short description from first user message
                        summary = m.sessionPreview(&s)
                }
                if len(summary) > 40 {
                        summary = summary[:37] + "..."
                }

                timeStr := s.UpdatedAt.Format("Jan 02 15:04")
                meta := fmt.Sprintf("%s  %s  %d msgs", timeStr, shortID, len(s.Messages))

                isSelected := (i == m.pickerSelect)
                innerPad := dialogWidth - 2 - 2 // -2 for left/right margin
                if isSelected {
                        // Selected row: highlight
                        entryLine := fmt.Sprintf(" ▸ %s", summary)
                        // Pad the entry to fill the dialog width
                        entryVisLen := len(fmt.Sprintf(" ▸ %s", summary))
                        metaVisLen := len(meta)
                        totalVis := entryVisLen + metaVisLen
                        if totalVis > innerPad {
                                if entryVisLen+3 > innerPad {
                                        entryLine = fmt.Sprintf(" ▸ %s", summary[:innerPad-3])
                                        entryVisLen = innerPad
                                }
                                meta = meta[:innerPad-entryVisLen]
                        }
                        pad := innerPad - len(entryLine) - len(meta)
                        if pad < 0 {
                                pad = 0
                        }
                        b.WriteString(selectedBg.Render(entryLine))
                        b.WriteString(strings.Repeat(" ", pad))
                        b.WriteString(selectedBg.Render(meta))
                } else {
                        entryLine := fmt.Sprintf("   %s", summary)
                        entryVisLen := len(entryLine)
                        metaVisLen := len(meta)
                        totalVis := entryVisLen + metaVisLen
                        if totalVis > innerPad {
                                if entryVisLen+3 > innerPad {
                                        entryLine = fmt.Sprintf("   %s", summary[:innerPad-3])
                                        entryVisLen = innerPad
                                }
                                meta = meta[:innerPad-entryVisLen]
                        }
                        pad := innerPad - len(entryLine) - len(meta)
                        if pad < 0 {
                                pad = 0
                        }
                        b.WriteString(summaryStyle.Render(entryLine))
                        b.WriteString(strings.Repeat(" ", pad))
                        b.WriteString(metaStyle.Render(meta))
                }

                // Pad to dialog width
                // This is tricky with ANSI; we just add the right border
                b.WriteString(borderStyle.Render("│"))
                b.WriteString("\n")
        }

        // If fewer sessions than visible height, fill with empty rows
        for i := endIdx; i < m.pickerScroll+visibleHeight; i++ {
                b.WriteString(horizSpace)
                b.WriteString(borderStyle.Render("│" + strings.Repeat(" ", dialogWidth-2) + "│"))
                b.WriteString("\n")
        }

        // Scroll indicator
        if numSessions > visibleHeight {
                b.WriteString(horizSpace)
                b.WriteString(borderStyle.Render("├" + strings.Repeat("─", dialogWidth-2) + "┤"))
                b.WriteString("\n")

                // Scroll bar
                b.WriteString(horizSpace)
                b.WriteString(borderStyle.Render("│"))
                scrollHint := dimStyle.Render(fmt.Sprintf("  ↑↓ navigate  Enter select  %d/%d  Esc cancel", m.pickerSelect+1, numSessions))
                scrollVisLen := len(fmt.Sprintf("  ↑↓ navigate  Enter select  %d/%d  Esc cancel", m.pickerSelect+1, numSessions))
                hintPad := dialogWidth - 2 - scrollVisLen
                if hintPad > 0 {
                        b.WriteString(strings.Repeat(" ", hintPad))
                } else if hintPad < 0 {
                        // Truncate - just show what fits
                        // We already wrote scrollHint, so it's fine if it overflows slightly
                }
                b.WriteString(scrollHint)
                b.WriteString(borderStyle.Render("│"))
                b.WriteString("\n")
        } else {
                // No scrollbar needed - just footer
                b.WriteString(horizSpace)
                b.WriteString(borderStyle.Render("├" + strings.Repeat("─", dialogWidth-2) + "┤"))
                b.WriteString("\n")

                b.WriteString(horizSpace)
                b.WriteString(borderStyle.Render("│"))
                hint := dimStyle.Render("  ↑↓ navigate  Enter select  Esc cancel")
                hintVisLen := len("  ↑↓ navigate  Enter select  Esc cancel")
                hintPad := dialogWidth - 2 - hintVisLen
                if hintPad > 0 {
                        b.WriteString(strings.Repeat(" ", hintPad))
                }
                b.WriteString(hint)
                b.WriteString(borderStyle.Render("│"))
                b.WriteString("\n")
        }

        // Bottom border
        b.WriteString(horizSpace)
        b.WriteString(borderStyle.Render("└" + strings.Repeat("─", dialogWidth-2) + "┘"))

        return b.String()
}

// sessionPreview generates a short preview string for a session.
func (m *replModel) sessionPreview(s *session.Session) string {
        for _, msg := range s.Messages {
                if msg.Role == "user" {
                        content := fmt.Sprintf("%v", msg.Content)
                        // Extract first line or first N chars
                        if len(content) > 50 {
                                content = content[:50]
                        }
                        // Remove newlines
                        content = strings.ReplaceAll(content, "\n", " ")
                        return content
                }
        }
        return "(empty session)"
}

// renderQuitDialog renders a centered quit confirmation dialog.
func (m *replModel) renderQuitDialog() string {
        dialogWidth := 40
        dialogHeight := 7

        // Calculate center position
        vertPad := (m.height - dialogHeight) / 2
        if vertPad < 0 {
                vertPad = 0
        }
        horizPad := (m.width - dialogWidth) / 2
        if horizPad < 0 {
                horizPad = 0
        }

        var b strings.Builder

        // Vertical padding to center
        for i := 0; i < vertPad; i++ {
                b.WriteString("\n")
        }

        // Horizontal padding
        horizSpace := strings.Repeat(" ", horizPad)

        // Box border style
        borderStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("63"))
        yellowStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("221")).Bold(true)
        dimStyle := lipgloss.NewStyle().Foreground(lipgloss.Color("245"))

        // Top border
        b.WriteString(horizSpace)
        b.WriteString(borderStyle.Render("┌" + strings.Repeat("─", dialogWidth-2) + "┌"))
        b.WriteString("\n")

        // Title line
        b.WriteString(horizSpace)
        title := "  Quit Cairn Code?  "
        titlePad := dialogWidth - 2 - len(title)
        if titlePad < 0 {
                titlePad = 0
        }
        b.WriteString(borderStyle.Render("│"))
        b.WriteString(yellowStyle.Render(title + strings.Repeat(" ", titlePad)))
        b.WriteString(borderStyle.Render("│"))
        b.WriteString("\n")

        // Empty line
        b.WriteString(horizSpace)
        b.WriteString(borderStyle.Render("│" + strings.Repeat(" ", dialogWidth-2) + "│"))
        b.WriteString("\n")

        // Options line: [ Yes ]  [ No ]
        b.WriteString(horizSpace)
        b.WriteString(borderStyle.Render("│"))
        innerPad := (dialogWidth - 2 - 18) / 2
        b.WriteString(strings.Repeat(" ", innerPad))

        yesStyle := promptStyle.Bold(true)
        noStyle := dimStyle
        if m.quitChoice == 0 {
                yesStyle = yellowStyle.Bold(true)
        } else {
                noStyle = yellowStyle.Bold(true)
        }
        b.WriteString(yesStyle.Render("◀ Yes ▶"))
        b.WriteString(dimStyle.Render("    "))
        b.WriteString(noStyle.Render("◀ No ▶"))

        remaining := dialogWidth - 2 - innerPad - 18
        if remaining > 0 {
                b.WriteString(strings.Repeat(" ", remaining))
        }
        b.WriteString(borderStyle.Render("│"))
        b.WriteString("\n")

        // Empty line
        b.WriteString(horizSpace)
        b.WriteString(borderStyle.Render("│" + strings.Repeat(" ", dialogWidth-2) + "│"))
        b.WriteString("\n")

        // Hint line
        b.WriteString(horizSpace)
        b.WriteString(borderStyle.Render("│"))
        hint := dimStyle.Render(" ← → to select, Enter to confirm ")
        hintPad := dialogWidth - 2 - len("[ ← → to select, Enter to confirm ]")
        if hintPad < 0 {
                hintPad = 0
        }
        // Strip ANSI to count visible chars for padding
        hintText := " ← → to select, Enter to confirm "
        visHint := len(hintText)
        totalPad := dialogWidth - 2 - visHint
        if totalPad < 0 {
                totalPad = 0
        }
        b.WriteString(strings.Repeat(" ", (totalPad)/2))
        b.WriteString(hint)
        b.WriteString(strings.Repeat(" ", totalPad - totalPad/2))
        b.WriteString(borderStyle.Render("│"))
        b.WriteString("\n")

        // Bottom border
        b.WriteString(horizSpace)
        b.WriteString(borderStyle.Render("└" + strings.Repeat("─", dialogWidth-2) + "┘"))

        return b.String()
}

// renderBanner renders the welcome banner (Claude Code style).
func (m *replModel) renderBanner() string {
        const innerWidth = 58 // matches the 58-dash border

        var b strings.Builder
        border := dimBorderStyle

        top := "╭" + strings.Repeat("─", innerWidth) + "╮"
        b.WriteString(border.Render(top))
        b.WriteString("\n")

        // Logo line: "  ⚡ Cairn Code v0.3.0"
        logo := fmt.Sprintf("  ⚡ Cairn Code v%s", m.version)
        b.WriteString(border.Render("│"))
        b.WriteString(brandStyle.Bold(true).Render(logo))
        b.WriteString(strings.Repeat(" ", innerWidth-lipgloss.Width(logo)))
        b.WriteString(border.Render("│"))
        b.WriteString("\n")

        // Tagline
        tag := "  open terminal coding agent"
        b.WriteString(border.Render("│"))
        b.WriteString(systemStyle.Render(tag))
        b.WriteString(strings.Repeat(" ", innerWidth-lipgloss.Width(tag)))
        b.WriteString(border.Render("│"))
        b.WriteString("\n")

        // Separator
        mid := "├" + strings.Repeat("─", innerWidth) + "┤"
        b.WriteString(border.Render(mid))
        b.WriteString("\n")

        // Model line
        if m.agent != nil {
                b.WriteString(border.Render("│"))
                modelLine := fmt.Sprintf("  Model   %s / %s", m.agent.ProviderName(), m.agent.Model())
                if lipgloss.Width(modelLine) > innerWidth {
                        modelLine = modelLine[:innerWidth-3] + "..."
                }
                b.WriteString(labelStyle.Render(modelLine))
                b.WriteString(strings.Repeat(" ", innerWidth-lipgloss.Width(modelLine)))
                b.WriteString(border.Render("│"))
                b.WriteString("\n")
        }

        // Path line
        b.WriteString(border.Render("│"))
        pathLine := fmt.Sprintf("  Path    %s", m.workDir)
        if lipgloss.Width(pathLine) > innerWidth {
                pathLine = pathLine[:innerWidth-3] + "..."
        }
        b.WriteString(labelStyle.Render(pathLine))
        b.WriteString(strings.Repeat(" ", innerWidth-lipgloss.Width(pathLine)))
        b.WriteString(border.Render("│"))
        b.WriteString("\n")

        // Bottom border
        bot := "╰" + strings.Repeat("─", innerWidth) + "╯"
        b.WriteString(border.Render(bot))

        return b.String()
}

// renderStreamingLine renders a single line from the streaming buffer through glamour.
// Used for complete lines during live streaming (not the final commit).
func (m *replModel) renderStreamingLine(line string) string {
        if m.renderer != nil {
                md, err := m.renderer.Render(line)
                if err == nil {
                        return md
                }
        }
        return line
}

// renderOutputLine renders a single output line.
func (m *replModel) renderOutputLine(line OutputLine) string {
        switch line.Type {
        case "user":
                return userStyle.Render("❯ " + line.Content) + "\n\n"

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
                // ● ToolName(description)
                b.WriteString(toolNameStyle.Render("● "))
                b.WriteString(toolNameStyle.Render(line.ToolName))
                if line.Content != "" {
                        // Truncate long tool inputs for display
                        desc := line.Content
                        if len(desc) > 80 {
                                desc = desc[:77] + "..."
                        }
                        b.WriteString("(")
                        b.WriteString(toolResultStyle.Render(desc))
                        b.WriteString(")")
                }
                b.WriteString("\n")
                return b.String()

        case "tool_result":
                var b strings.Builder
                b.WriteString(successStyle.Render("● "))
                b.WriteString(toolResultStyle.Render(fmt.Sprintf("%s", line.ToolName)))
                if line.Duration > 0 {
                        b.WriteString(usageStyle.Render(fmt.Sprintf(" (%.1fs)", line.Duration.Seconds())))
                }
                b.WriteString("\n")
                // Show truncated output for tool results
                content := strings.TrimSpace(line.Content)
                if content != "" {
                        if len(content) > 500 {
                                content = content[:500] + "\n  ... [truncated]"
                        }
                        b.WriteString(toolResultStyle.Render(indent(content, "  ")))
                        b.WriteString("\n")
                }
                return b.String()

        case "error":
                return errorStyle.Render("● " + line.Content) + "\n\n"

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
                m.currentVerb = pickSpinnerVerb()
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
                if len(parts) > 1 {
                        // Resume specific session by ID
                        resumeID := parts[1]
                        m.state = stateRunning
                        m.currentVerb = pickSpinnerVerb()
                        return m, tea.Batch(m.resumeSession(resumeID), tickSpinner())
                }
                // No ID given — show session picker
                m.state = stateRunning
                m.currentVerb = pickSpinnerVerb()
                return m, tea.Batch(m.loadSessionsForPicker(), tickSpinner())

        case "/sessions":
                m.state = stateRunning
                m.currentVerb = pickSpinnerVerb()
                return m, tea.Batch(m.listSessions(), tickSpinner())

        case "/tools":
                var buf strings.Builder
                toolNames := m.agent.ToolNames()
                buf.WriteString(fmt.Sprintf("Available tools (%d):\n\n", len(toolNames)))
                for _, name := range toolNames {
                        desc := toolDescriptions[name]
                        if desc == "" {
                                desc = "(no description)"
                        }
                        buf.WriteString(fmt.Sprintf("  ● %-25s %s\n", name, desc))
                }
                m.output = append(m.output, OutputLine{
                        Type:    "system",
                        Content: buf.String(),
                })
                return m, nil

        case "/exit", "/quit", "/q":
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

// runBashCommand runs a shell command directly (bash mode via ! prefix).
func (m replModel) runBashCommand(cmd string) tea.Cmd {
        return func() tea.Msg {
                start := time.Now()
                c := exec.CommandContext(context.Background(), "bash", "-c", cmd)
                output, err := c.CombinedOutput()
                duration := time.Since(start)

                var lines []OutputLine
                lines = append(lines, OutputLine{
                        Type:     "tool_use",
                        ToolName: "bash",
                        Content:  cmd,
                })

                resultStr := string(output)
                if len(resultStr) > 2000 {
                        resultStr = resultStr[:2000] + "\n  ... [truncated]"
                }

                if err != nil {
                        lines = append(lines, OutputLine{
                                Type:     "error",
                                Content:  fmt.Sprintf("bash: %s", err.Error()),
                        })
                        if resultStr != "" {
                                lines = append(lines, OutputLine{
                                        Type:     "tool_result",
                                        ToolName: "bash",
                                        Content:  resultStr,
                                        Duration: duration,
                                })
                        }
                } else {
                        lines = append(lines, OutputLine{
                                Type:     "tool_result",
                                ToolName: "bash",
                                Content:  resultStr,
                                Duration: duration,
                        })
                }

                return agentCompleteMsg{
                        output: lines,
                        usage:  llm.Usage{},
                }
        }
}

// runAgent starts the agent in a goroutine with a channel for streaming updates.
func (m replModel) runAgent(input string) tea.Cmd {
        // Create channel for streaming chunks — stored in model for polling
        chunkCh := make(chan string, 256)
        resultCh := make(chan agentCompleteMsg, 1)

        return func() tea.Msg {
                // Store channels in model for polling
                m.streamChunkCh = chunkCh
                m.streamResultCh = resultCh

                // Start agent in background goroutine
                go func() {
                        var collectedOutput []OutputLine
                        var totalUsage llm.Usage

                        cb := agent.Callbacks{
                                OnText: func(text string) {
                                        // Final committed text (full response after streaming done)
                                        collectedOutput = append(collectedOutput, OutputLine{
                                                Type:    "text",
                                                Content: text,
                                        })
                                        // Signal UI to clear streaming buffer
                                        select {
                                        case chunkCh <- "\x00STREAM_CLEAR\x00":
                                        default:
                                        }
                                },
                                OnStreamChunk: func(chunk string) {
                                        select {
                                        case chunkCh <- chunk:
                                        default:
                                        }
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
                        agentErr := a.Run(context.Background(), input)

                        resultCh <- agentCompleteMsg{
                                output: collectedOutput,
                                usage:  totalUsage,
                                err:    agentErr,
                        }
                        close(chunkCh)
                }()

                // Start draining chunks immediately
                return drainStreamMsg{}
        }
}

// drainStreamMsg triggers non-blocking drain of the streaming chunk channel.
type drainStreamMsg struct{}

// drainStreamTicks returns a command that polls the chunk channel at ~60fps.
func drainStreamTicks() tea.Cmd {
        return tea.Tick(time.Millisecond*16, func(t time.Time) tea.Msg {
                return drainStreamMsg{}
        })
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

                // Rebuild output buffer from session history so it looks like
                // the conversation happened in this terminal session
                var historyOutput []OutputLine
                for _, sm := range sess.Messages {
                        switch sm.Role {
                        case "user":
                                text := extractTextFromContent(sm.Content)
                                if text != "" {
                                        historyOutput = append(historyOutput, OutputLine{
                                                Type:    "user",
                                                Content: text,
                                        })
                                }
                        case "assistant":
                                lines := renderAssistantMessage(sm.Content)
                                historyOutput = append(historyOutput, lines...)
                        }
                }

                // Build result: history lines + resume confirmation
                allOutput := historyOutput
                allOutput = append(allOutput, OutputLine{
                        Type:    "system",
                        Content: fmt.Sprintf("Resumed session %s (model: %s, messages: %d)", sess.ID, sess.Model, len(sess.Messages)),
                })

                return agentCompleteMsg{
                        output: allOutput,
                }
        }
}

// loadSessionsForPicker loads sessions and returns them via a message for the picker UI.
func (m replModel) loadSessionsForPicker() tea.Cmd {
        return func() tea.Msg {
                sessions, err := session.ListSessions(m.sessionDir)
                return sessionsLoadedMsg{sessions: sessions, err: err}
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

type cursorBlinkMsg time.Time

type agentCompleteMsg struct {
        output []OutputLine
        usage  llm.Usage
        err    error
}

type sessionsLoadedMsg struct {
        sessions []session.Session
        err      error
}

// tickSpinner returns a command that ticks the spinner.
func tickSpinner() tea.Cmd {
        return tea.Tick(time.Millisecond*80, func(t time.Time) tea.Msg {
                return spinnerTickMsg(t)
        })
}

// pickSpinnerVerb randomly selects a spinner verb (once per turn, Claude Code style).
func pickSpinnerVerb() string {
        return spinnerVerbs[rand.IntN(len(spinnerVerbs))]
}

// extractTextFromContent extracts plain text from a message content field.
// Content can be a string, []llm.ContentBlock, or []any (from JSON unmarshal).
func extractTextFromContent(content any) string {
        switch c := content.(type) {
        case string:
                return c
        case []llm.ContentBlock:
                var parts []string
                for _, b := range c {
                        if b.Type == "text" && b.Text != "" {
                                parts = append(parts, b.Text)
                        }
                }
                return strings.Join(parts, "\n")
        case []any:
                // From JSON deserialization — ContentBlock as maps
                var parts []string
                for _, item := range c {
                        if m, ok := item.(map[string]any); ok {
                                if t, ok := m["type"].(string); ok && t == "text" {
                                        if text, ok := m["text"].(string); ok && text != "" {
                                                parts = append(parts, text)
                                        }
                                }
                        }
                }
                return strings.Join(parts, "\n")
        default:
                return fmt.Sprintf("%v", c)
        }
}

// renderAssistantMessage converts an assistant message's content into OutputLines
// for replay in the terminal buffer (text and tool use/result blocks).
func renderAssistantMessage(content any) []OutputLine {
        var blocks []llm.ContentBlock
        switch c := content.(type) {
        case []llm.ContentBlock:
                blocks = c
        case []any:
                blocks = llm.AsTextBlocks(c)
        case string:
                if c != "" {
                        return []OutputLine{{Type: "text", Content: c}}
                }
                return nil
        default:
                return nil
        }

        var lines []OutputLine
        for _, b := range blocks {
                switch b.Type {
                case "text":
                        if b.Text != "" {
                                lines = append(lines, OutputLine{Type: "text", Content: b.Text})
                        }
                case "tool_use":
                        name := b.Name
                        if name == "" {
                                name = "unknown"
                        }
                        inputStr := formatToolInput(b.Input)
                        lines = append(lines, OutputLine{
                                Type:     "tool_use",
                                ToolName: name,
                                Content:  inputStr,
                        })
                case "tool_result":
                        name := b.ID // tool_result uses ID as the tool call ID reference
                        resultText := b.Content
                        if resultText != "" {
                                if len(resultText) > 500 {
                                        resultText = resultText[:500] + "\n  ... [truncated]"
                                }
                                lines = append(lines, OutputLine{
                                        Type:     "tool_result",
                                        ToolName: name,
                                        Content:  resultText,
                                })
                        }
                }
        }
        return lines
}

// tickCursorBlink returns a command that blinks the cursor every 530ms.
func tickCursorBlink() tea.Cmd {
        return tea.Tick(time.Millisecond*530, func(t time.Time) tea.Msg {
                return cursorBlinkMsg(t)
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
