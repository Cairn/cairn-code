package diff

import (
	"strings"
	"testing"
)

// TestDiffIdentical verifies identical strings produce only context lines.
func TestDiffIdentical(t *testing.T) {
	lines := Compute("line1\nline2\nline3\n", "line1\nline2\nline3\n")
	added, removed, unchanged := Stats(lines)

	if added != 0 || removed != 0 {
		t.Errorf("identical strings: added=%d removed=%d, want 0/0", added, removed)
	}
	if unchanged != 3 {
		t.Errorf("unchanged=%d, want 3", unchanged)
	}
}

// TestDiffEmptyToContent verifies empty to non-empty produces all additions.
func TestDiffEmptyToContent(t *testing.T) {
	lines := Compute("", "line1\nline2\n")
	added, removed, _ := Stats(lines)

	if added != 2 {
		t.Errorf("added=%d, want 2", added)
	}
	if removed != 0 {
		t.Errorf("removed=%d, want 0", removed)
	}
}

// TestDiffContentToEmpty verifies non-empty to empty produces all removals.
func TestDiffContentToEmpty(t *testing.T) {
	lines := Compute("line1\nline2\n", "")
	added, removed, _ := Stats(lines)

	if added != 0 {
		t.Errorf("added=%d, want 0", added)
	}
	if removed != 2 {
		t.Errorf("removed=%d, want 2", removed)
	}
}

// TestDiffBothEmpty verifies both empty produces only header.
func TestDiffBothEmpty(t *testing.T) {
	lines := Compute("", "")
	added, removed, unchanged := Stats(lines)

	if added != 0 || removed != 0 || unchanged != 0 {
		t.Errorf("both empty: added=%d removed=%d unchanged=%d, want 0/0/0", added, removed, unchanged)
	}
}

// TestDiffSingleLineChange verifies a single modified line.
func TestDiffSingleLineChange(t *testing.T) {
	lines := Compute("line1\nline2\nline3\n", "line1\nCHANGED\nline3\n")
	added, removed, _ := Stats(lines)

	if added != 1 {
		t.Errorf("added=%d, want 1", added)
	}
	if removed != 1 {
		t.Errorf("removed=%d, want 1", removed)
	}
}

// TestDiffAddAtEnd verifies adding lines at the end.
func TestDiffAddAtEnd(t *testing.T) {
	lines := Compute("line1\n", "line1\nline2\nline3\n")
	added, removed, _ := Stats(lines)

	if added != 2 {
		t.Errorf("added=%d, want 2", added)
	}
	if removed != 0 {
		t.Errorf("removed=%d, want 0", removed)
	}
}

// TestDiffRemoveFromEnd verifies removing lines from the end.
func TestDiffRemoveFromEnd(t *testing.T) {
	lines := Compute("line1\nline2\nline3\n", "line1\n")
	added, removed, _ := Stats(lines)

	if added != 0 {
		t.Errorf("added=%d, want 0", added)
	}
	if removed != 2 {
		t.Errorf("removed=%d, want 2", removed)
	}
}

// TestDiffAddAtBeginning verifies adding lines at the beginning.
func TestDiffAddAtBeginning(t *testing.T) {
	lines := Compute("line2\nline3\n", "line1\nline2\nline3\n")
	added, removed, _ := Stats(lines)

	if added != 1 {
		t.Errorf("added=%d, want 1", added)
	}
	if removed != 0 {
		t.Errorf("removed=%d, want 0", removed)
	}
}

// TestDiffMultipleChanges verifies scattered changes.
func TestDiffMultipleChanges(t *testing.T) {
	old := "a\nb\nc\nd\ne\n"
	new := "a\nX\nc\nY\ne\n"
	lines := Compute(old, new)
	added, removed, _ := Stats(lines)

	if added != 2 {
		t.Errorf("added=%d, want 2", added)
	}
	if removed != 2 {
		t.Errorf("removed=%d, want 2", removed)
	}
}

// TestDiffLineNumbers verifies line numbers are correct.
func TestDiffLineNumbers(t *testing.T) {
	old := "old1\nold2\nold3\n"
	new := "new1\nold2\nnew3\n"
	lines := Compute(old, new)

	// Find the context line (should be "old2" at line 2 in both)
	foundContext := false
	for _, line := range lines {
		if line.Type == "context" && line.Text == "old2" {
			if line.OldNum != 2 || line.NewNum != 2 {
				t.Errorf("context line old2: OldNum=%d NewNum=%d, want 2/2", line.OldNum, line.NewNum)
			}
			foundContext = true
		}
	}
	if !foundContext {
		t.Error("expected to find context line 'old2'")
	}
}

// TestDiffDuplicateLines verifies LCS handles duplicate lines.
func TestDiffDuplicateLines(t *testing.T) {
	old := "dup\ndup\nline1\n"
	new := "dup\ndup\nline2\n"
	lines := Compute(old, new)
	added, removed, _ := Stats(lines)

	if added != 1 {
		t.Errorf("added=%d, want 1", added)
	}
	if removed != 1 {
		t.Errorf("removed=%d, want 1", removed)
	}
}

// TestFormatUnified verifies unified diff formatting.
func TestFormatUnified(t *testing.T) {
	lines := []DiffLine{
		{Type: "header", Text: "--- original"},
		{Type: "remove", OldNum: 1, Text: "old line"},
		{Type: "add", NewNum: 1, Text: "new line"},
		{Type: "context", OldNum: 2, NewNum: 2, Text: "same"},
	}

	result := FormatUnified(lines, "old.txt", "new.txt")

	if !strings.Contains(result, "--- old.txt") {
		t.Errorf("missing old file header: %q", result)
	}
	if !strings.Contains(result, "+++ new.txt") {
		t.Errorf("missing new file header: %q", result)
	}
	if !strings.HasPrefix(result, "--- old.txt\n+++ new.txt\n") {
		t.Errorf("headers should come first: %q", result)
	}
}

// TestFormatUnifiedEmpty verifies empty diff produces header only.
func TestFormatUnifiedEmpty(t *testing.T) {
	result := FormatUnified([]DiffLine{}, "a", "b")
	if !strings.Contains(result, "--- a\n+++ b") {
		t.Errorf("expected headers even for empty diff: %q", result)
	}
}

// TestStats verifies stats counting.
func TestStats(t *testing.T) {
	lines := []DiffLine{
		{Type: "header", Text: "---"},
		{Type: "add", Text: "new"},
		{Type: "add", Text: "new2"},
		{Type: "remove", Text: "old"},
		{Type: "context", Text: "same"},
		{Type: "context", Text: "same2"},
	}

	added, removed, unchanged := Stats(lines)
	if added != 2 {
		t.Errorf("added=%d, want 2", added)
	}
	if removed != 1 {
		t.Errorf("removed=%d, want 1", removed)
	}
	if unchanged != 2 {
		t.Errorf("unchanged=%d, want 2", unchanged)
	}
}

// TestComputeLCS verifies LCS computation directly.
func TestComputeLCS(t *testing.T) {
	old := []string{"a", "b", "c"}
	new := []string{"a", "x", "c"}

	lcs := computeLCS(old, new)
	if len(lcs) != 2 {
		t.Fatalf("LCS length = %d, want 2", len(lcs))
	}
	if lcs[0].Old != 0 || lcs[0].New != 0 {
		t.Errorf("LCS[0] = {%d,%d}, want {0,0}", lcs[0].Old, lcs[0].New)
	}
	if lcs[1].Old != 2 || lcs[1].New != 2 {
		t.Errorf("LCS[1] = {%d,%d}, want {2,2}", lcs[1].Old, lcs[1].New)
	}
}

// TestComputeLCSIdentical verifies LCS of identical sequences.
func TestComputeLCSIdentical(t *testing.T) {
	seq := []string{"a", "b", "c", "d"}
	lcs := computeLCS(seq, seq)

	if len(lcs) != 4 {
		t.Fatalf("LCS of identical = %d, want 4", len(lcs))
	}
}

// TestComputeLCSNoCommon verifies LCS with no common elements.
func TestComputeLCSNoCommon(t *testing.T) {
	lcs := computeLCS([]string{"a", "b"}, []string{"c", "d"})
	if len(lcs) != 0 {
		t.Errorf("LCS of disjoint = %d, want 0", len(lcs))
	}
}

// TestDiffTrailingNewline verifies trailing newline handling.
func TestDiffTrailingNewline(t *testing.T) {
	// Both with trailing newline
	lines1 := Compute("hello\n", "hello\n")
	added1, removed1, _ := Stats(lines1)
	if added1 != 0 || removed1 != 0 {
		t.Errorf("trailing newline: added=%d removed=%d", added1, removed1)
	}

	// Without trailing newline
	lines2 := Compute("hello", "hello")
	added2, removed2, _ := Stats(lines2)
	if added2 != 0 || removed2 != 0 {
		t.Errorf("no trailing newline: added=%d removed=%d", added2, removed2)
	}
}
