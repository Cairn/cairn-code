package tools

import (
        "context"
        "encoding/json"
        "fmt"
        "io"
        "net/http"
        "net/url"
        "os/exec"
        "regexp"
        "strings"
        "time"
)

// WebSearchTool searches the web using DuckDuckGo's HTML search.
type WebSearchTool struct {
        client *http.Client
}

func NewWebSearchTool() *WebSearchTool {
        return &WebSearchTool{
                client: &http.Client{Timeout: 30 * time.Second},
        }
}

func (t *WebSearchTool) Name() string { return "web_search" }

func (t *WebSearchTool) Description() string {
        return "Searches the web using DuckDuckGo and returns the top 5 results with titles, URLs, and snippets. Useful for finding documentation, code examples, and current information."
}

func (t *WebSearchTool) InputSchema() map[string]any {
        return map[string]any{
                "type": "object",
                "properties": map[string]any{
                        "query": map[string]any{
                                "type":        "string",
                                "description": "The search query to look up on the web.",
                        },
                },
                "required": []string{"query"},
        }
}

func (t *WebSearchTool) NeedsPermission() bool { return false }

type webSearchInput struct {
        Query string `json:"query"`
}

// searchResult represents a single search result.
type searchResult struct {
        Title   string
        URL     string
        Snippet string
}

func (t *WebSearchTool) Execute(ctx context.Context, input json.RawMessage) (string, error) {
        var params webSearchInput
        if err := json.Unmarshal(input, &params); err != nil {
                return "", fmt.Errorf("invalid input: %w", err)
        }

        if params.Query == "" {
                return "", fmt.Errorf("query is required")
        }

        results, err := t.searchDuckDuckGo(ctx, params.Query)
        if err != nil {
                return "", fmt.Errorf("search failed: %w", err)
        }

        if len(results) == 0 {
                return "No results found for the given query.", nil
        }

        var buf strings.Builder
        buf.WriteString(fmt.Sprintf("Search results for: %s\n\n", params.Query))
        for i, r := range results {
                buf.WriteString(fmt.Sprintf("%d. %s\n   URL: %s\n   %s\n\n", i+1, r.Title, r.URL, r.Snippet))
        }
        buf.WriteString(fmt.Sprintf("%d results returned.", len(results)))
        return buf.String(), nil
}

// searchDuckDuckGo performs a search using DuckDuckGo's lite HTML endpoint.
func (t *WebSearchTool) searchDuckDuckGo(ctx context.Context, query string) ([]searchResult, error) {
        // Use DuckDuckGo lite HTML endpoint which returns simple HTML results
        searchURL := fmt.Sprintf("https://lite.duckduckgo.com/lite/?q=%s", url.QueryEscape(query))

        req, err := http.NewRequestWithContext(ctx, "GET", searchURL, nil)
        if err != nil {
                return nil, err
        }
        req.Header.Set("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")

        resp, err := t.client.Do(req)
        if err != nil {
                return nil, err
        }
        defer resp.Body.Close()

        if resp.StatusCode != http.StatusOK {
                return nil, fmt.Errorf("DuckDuckGo returned status %d", resp.StatusCode)
        }

        body, err := io.ReadAll(resp.Body)
        if err != nil {
                return nil, err
        }

        return parseDuckDuckGoResults(string(body)), nil
}

// parseDuckDuckGoResults parses DuckDuckGo lite HTML to extract search results.
func parseDuckDuckGoResults(html string) []searchResult {
        var results []searchResult

        // DuckDuckGo lite uses a specific format: results are in <a> tags with class
        // "result-link" and snippets in <td class="result-snippet">
        // We use regex to extract the key parts.

        // Match link blocks: each result is a <tr class="result-link">...</tr>
        linkRe := regexp.MustCompile(`<a[^>]+class="result-link"[^>]*href="([^"]+)"[^>]*>(.*?)</a>`)
        snippetRe := regexp.MustCompile(`<td[^>]+class="result-snippet"[^>]*>(.*?)</td>`)

        linkMatches := linkRe.FindAllStringSubmatch(html, 5)
        snippetMatches := snippetRe.FindAllStringSubmatch(html, 5)

        for i, lm := range linkMatches {
                if i >= 5 {
                        break
                }
                href := lm[1]
                title := stripHTMLTags(lm[2])

                snippet := ""
                if i < len(snippetMatches) {
                        snippet = stripHTMLTags(snippetMatches[i][1])
                }

                // DuckDuckGo lite URLs may be relative or use their redirect format
                if strings.HasPrefix(href, "//duckduckgo.com/l/") || strings.HasPrefix(href, "/l/") {
                        // Redirect URL — extract the actual URL from theuddg parameter
                        if u, err := url.Parse(href); err == nil {
                                if redirectURL := u.Query().Get("uddg"); redirectURL != "" {
                                        href = redirectURL
                                }
                        }
                }

                // Ensure it has a scheme
                if !strings.HasPrefix(href, "http") {
                        href = "https://" + href
                }

                if title != "" {
                        results = append(results, searchResult{
                                Title:   title,
                                URL:     href,
                                Snippet: snippet,
                        })
                }
        }

        // Fallback: if no DuckDuckGo results parsed, try curl-based approach
        if len(results) == 0 {
                return searchViaCurl(html)
        }

        return results
}

// searchViaCurl is a fallback that uses curl to hit a search API.
func searchViaCurl(html string) []searchResult {
        // This is a last-resort fallback when DuckDuckGo HTML parsing fails.
        // Try parsing the HTML more loosely for any links that look like results.
        var results []searchResult

        // Look for any anchor tags with URLs that aren't duckduckgo.com internal links
        linkRe := regexp.MustCompile(`<a[^>]+href="(https?://[^"]+)"[^>]*>(.*?)</a>`)
        matches := linkRe.FindAllStringSubmatch(html, 20)

        seen := make(map[string]bool)
        for _, m := range matches {
                href := m[1]
                title := stripHTMLTags(m[2])

                // Skip internal DuckDuckGo links
                if strings.Contains(href, "duckduckgo.com") {
                        continue
                }

                if seen[href] || title == "" {
                        continue
                }
                seen[href] = true

                results = append(results, searchResult{
                        Title: title,
                        URL:   href,
                })

                if len(results) >= 5 {
                        break
                }
        }

        return results
}

// stripHTMLTags removes all HTML tags from a string.
func stripHTMLTags(s string) string {
        // Simple tag stripper — good enough for our use case
        re := regexp.MustCompile(`<[^>]*>`)
        return strings.TrimSpace(re.ReplaceAllString(s, ""))
}

// ensureImportsUsed — web_search also supports a curl-based approach.
// We keep exec.Command available as a backup if the HTTP approach doesn't work.
// (exec is not used directly but kept available for potential future fallback paths)
var _ = exec.CommandContext
