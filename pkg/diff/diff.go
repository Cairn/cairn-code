package diff

import (
	"fmt"
	"strings"
)

// DiffLine represents a single line in a diff output.
type DiffLine struct {
	Type   string // "context", "add", "remove", "header"
	OldNum int    // line number in old file (0 if not applicable)
	NewNum int    // line number in new file (0 if not applicable)
	Text   string
}

// Compute computes a unified diff between two strings.
func Compute(old, new string) []DiffLine {
	oldLines := strings.Split(old, "\n")
	newLines := strings.Split(new, "\n")

	// Remove trailing empty strings from split
	if len(oldLines) > 0 && oldLines[len(oldLines)-1] == "" {
		oldLines = oldLines[:len(oldLines)-1]
	}
	if len(newLines) > 0 && newLines[len(newLines)-1] == "" {
		newLines = newLines[:len(newLines)-1]
	}

	// Simple LCS-based diff using dynamic programming
	lcs := computeLCS(oldLines, newLines)

	var result []DiffLine
	result = append(result, DiffLine{Type: "header", Text: "--- original"})

	oldIdx := 0
	newIdx := 0
	oldLineNum := 0
	newLineNum := 0

	for _, pair := range lcs {
		// Output removed lines before this match
		for oldIdx < pair.Old {
			oldLineNum++
			result = append(result, DiffLine{
				Type:   "remove",
				OldNum: oldLineNum,
				Text:   oldLines[oldIdx],
			})
			oldIdx++
		}

		// Output added lines before this match
		for newIdx < pair.New {
			newLineNum++
			result = append(result, DiffLine{
				Type:   "add",
				NewNum: newLineNum,
				Text:   newLines[newIdx],
			})
			newIdx++
		}

		// Output the matching line
		oldLineNum++
		newLineNum++
		result = append(result, DiffLine{
			Type:   "context",
			OldNum: oldLineNum,
			NewNum: newLineNum,
			Text:   oldLines[oldIdx],
		})
		oldIdx++
		newIdx++
	}

	// Output remaining removed lines
	for oldIdx < len(oldLines) {
		oldLineNum++
		result = append(result, DiffLine{
			Type:   "remove",
			OldNum: oldLineNum,
			Text:   oldLines[oldIdx],
		})
		oldIdx++
	}

	// Output remaining added lines
	for newIdx < len(newLines) {
		newLineNum++
		result = append(result, DiffLine{
			Type:   "add",
			NewNum: newLineNum,
			Text:   newLines[newIdx],
		})
		newIdx++
	}

	return result
}

// lcsEntry represents a position pair in the LCS.
type lcsEntry struct {
	Old int
	New int
}

// computeLCS computes the Longest Common Subsequence using dynamic programming.
func computeLCS(old, new []string) []lcsEntry {
	m := len(old)
	n := len(new)

	// Build LCS table
	dp := make([][]int, m+1)
	for i := range dp {
		dp[i] = make([]int, n+1)
	}

	for i := 1; i <= m; i++ {
		for j := 1; j <= n; j++ {
			if old[i-1] == new[j-1] {
				dp[i][j] = dp[i-1][j-1] + 1
			} else if dp[i-1][j] > dp[i][j-1] {
				dp[i][j] = dp[i-1][j]
			} else {
				dp[i][j] = dp[i][j-1]
			}
		}
	}

	// Backtrack to find the LCS entries
	var result []lcsEntry
	i, j := m, n
	for i > 0 && j > 0 {
		if old[i-1] == new[j-1] {
			result = append(result, lcsEntry{Old: i - 1, New: j - 1})
			i--
			j--
		} else if dp[i-1][j] > dp[i][j-1] {
			i--
		} else {
			j--
		}
	}

	// Reverse since we built it backwards
	for l, r := 0, len(result)-1; l < r; l, r = l+1, r-1 {
		result[l], result[r] = result[r], result[l]
	}

	return result
}

// FormatUnified formats diff lines as a unified diff string.
func FormatUnified(lines []DiffLine, oldFile, newFile string) string {
	var buf strings.Builder
	fmt.Fprintf(&buf, "--- %s\n", oldFile)
	fmt.Fprintf(&buf, "+++ %s\n", newFile)

	for _, line := range lines {
		switch line.Type {
		case "header":
			fmt.Fprintf(&buf, "%s\n", line.Text)
		case "add":
			fmt.Fprintf(&buf, "+%s\n", line.Text)
		case "remove":
			fmt.Fprintf(&buf, "-%s\n", line.Text)
		case "context":
			fmt.Fprintf(&buf, " %s\n", line.Text)
		}
	}

	return buf.String()
}

// Stats returns summary statistics about a diff.
func Stats(lines []DiffLine) (added, removed, unchanged int) {
	for _, line := range lines {
		switch line.Type {
		case "add":
			added++
		case "remove":
			removed++
		case "context":
			unchanged++
		}
	}
	return
}
