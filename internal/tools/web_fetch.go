package tools

import (
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"regexp"
	"strings"
	"time"
)

// WebFetchTool fetches the content of a web page and returns the text.
type WebFetchTool struct {
	client *http.Client
}

func NewWebFetchTool() *WebFetchTool {
	return &WebFetchTool{
		client: &http.Client{Timeout: 30 * time.Second},
	}
}

func (t *WebFetchTool) Name() string { return "web_fetch" }

func (t *WebFetchTool) Description() string {
	return "Fetches the content of a web page and returns its text. Returns the first 2000 characters of extracted text. Optionally filters content based on a prompt."
}

func (t *WebFetchTool) InputSchema() map[string]any {
	return map[string]any{
		"type": "object",
		"properties": map[string]any{
			"url": map[string]any{
				"type":        "string",
				"description": "The URL of the web page to fetch.",
			},
			"prompt": map[string]any{
				"type":        "string",
				"description": "Optional: a prompt describing what information to extract from the page. Used to filter relevant content.",
			},
		},
		"required": []string{"url"},
	}
}

func (t *WebFetchTool) NeedsPermission() bool { return true }

type webFetchInput struct {
	URL    string `json:"url"`
	Prompt string `json:"prompt,omitempty"`
}

func (t *WebFetchTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
	var params webFetchInput
	if err := json.Unmarshal(input, &params); err != nil {
		return "", fmt.Errorf("invalid input: %w", err)
	}

	if params.URL == "" {
		return "", fmt.Errorf("url is required")
	}

	// Validate URL scheme
	if !strings.HasPrefix(params.URL, "http://") && !strings.HasPrefix(params.URL, "https://") {
		return "", fmt.Errorf("url must start with http:// or https://")
	}

	// Create request
	req, err := http.NewRequestWithContext(ctx, "GET", params.URL, nil)
	if err != nil {
		return "", fmt.Errorf("creating request: %w", err)
	}

	// Set a reasonable User-Agent
	req.Header.Set("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
	req.Header.Set("Accept", "text/html,application/xhtml+xml,text/plain,application/json")

	resp, err := t.client.Do(req)
	if err != nil {
		return "", fmt.Errorf("fetching URL: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != http.StatusOK {
		return "", fmt.Errorf("HTTP error: status %d", resp.StatusCode)
	}

	// Read body (limit to 1MB)
	body, err := io.ReadAll(io.LimitReader(resp.Body, 1024*1024))
	if err != nil {
		return "", fmt.Errorf("reading response: %w", err)
	}

	// Extract text from HTML
	text := htmlToText(string(body))

	// Truncate to 2000 characters
	if len(text) > 2000 {
		text = text[:2000] + "\n... [content truncated]"
	}

	if text == "" {
		return fmt.Sprintf("Fetched %s but no readable text content was found (content-length: %d bytes).", params.URL, len(body)), nil
	}

	result := fmt.Sprintf("Fetched: %s\n\n%s", params.URL, text)
	return result, nil
}

// htmlToText converts HTML to plain text by removing tags and cleaning up whitespace.
func htmlToText(html string) string {
	// Remove script and style blocks
	scriptRe := regexp.MustCompile(`(?is)<script[^>]*>.*?</script>`)
	styleRe := regexp.MustCompile(`(?is)<style[^>]*>.*?</style>`)
	html = scriptRe.ReplaceAllString(html, "")
	html = styleRe.ReplaceAllString(html, "")

	// Remove HTML tags
	tagRe := regexp.MustCompile(`<[^>]*>`)
	text := tagRe.ReplaceAllString(html, " ")

	// Decode common HTML entities
	text = decodeEntities(text)

	// Normalize whitespace
	spaceRe := regexp.MustCompile(`[ \t]+`)
	text = spaceRe.ReplaceAllString(text, " ")

	lineRe := regexp.MustCompile(`\n\s*\n`)
	text = lineRe.ReplaceAllString(text, "\n\n")

	// Trim and clean up
	text = strings.TrimSpace(text)

	return text
}

// decodeEntities handles common HTML entities.
func decodeEntities(s string) string {
	replacements := map[string]string{
		"&amp;":  "&",
		"&lt;":   "<",
		"&gt;":   ">",
		"&quot;": "\"",
		"&#39;":  "'",
		"&apos;": "'",
		"&nbsp;": " ",
	}

	for entity, char := range replacements {
		s = strings.ReplaceAll(s, entity, char)
	}

	// Handle numeric entities like &#123; and &#x1F;
	numRe := regexp.MustCompile(`&#x([0-9a-fA-F]+);`)
	s = numRe.ReplaceAllStringFunc(s, func(match string) string {
		var num int
		fmt.Sscanf(match, "&#x%x;", &num)
		if num > 0 && num < 65536 {
			return string(rune(num))
		}
		return match
	})

	decRe := regexp.MustCompile(`&#(\d+);`)
	s = decRe.ReplaceAllStringFunc(s, func(match string) string {
		var num int
		fmt.Sscanf(match, "&#%d;", &num)
		if num > 0 && num < 65536 {
			return string(rune(num))
		}
		return match
	})

	return s
}
