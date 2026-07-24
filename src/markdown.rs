use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SynStyle, Theme as SyntectTheme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use crate::theme::Theme;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static SYNTECT_THEME: OnceLock<SyntectTheme> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn syntect_theme() -> &'static SyntectTheme {
    SYNTECT_THEME.get_or_init(|| {
        let themes = ThemeSet::load_defaults();
        themes
            .themes
            .get("base16-ocean.dark")
            .or_else(|| themes.themes.get("Solarized (dark)"))
            .cloned()
            .unwrap_or_else(|| {
                themes
                    .themes
                    .values()
                    .next()
                    .cloned()
                    .expect("syntect ships at least one theme")
            })
    })
}

/// Semantic markdown styles derived from the active TUI theme so inline paths
/// and list chrome match the composer prompt colour (and friends).
#[derive(Clone, Copy)]
struct MdStyles {
    /// Inline `` `code` `` / file paths — same as prompt `❯` (accent_fg).
    inline_code: Style,
    /// Unordered bullet and ordered list prefix.
    list_marker: Style,
    /// Markdown links.
    link: Style,
    heading1: Style,
    heading2: Style,
    heading3: Style,
    heading_rest: Style,
    quote_bar: Style,
    quote_text: Style,
    /// Fallback colour for unhighlighted fenced code.
    code_mono: Style,
}

impl MdStyles {
    fn from_theme(theme: &Theme) -> Self {
        Self {
            // Match composer prompt colour so listed files read as themed chrome.
            inline_code: theme.accent_fg,
            list_marker: theme.accent_fg,
            link: theme.blue.add_modifier(Modifier::UNDERLINED),
            heading1: theme.amber.add_modifier(Modifier::BOLD),
            heading2: theme.accent_fg.add_modifier(Modifier::BOLD),
            heading3: theme.accent_fg,
            heading_rest: theme.muted,
            quote_bar: theme.faintest,
            quote_text: theme.faint,
            code_mono: theme.green,
        }
    }
}

/// Render markdown using the active TUI theme for inline chrome (code, lists, links).
pub fn render(text: &str, theme: &Theme) -> Vec<Line<'static>> {
    render_with(text, MdStyles::from_theme(theme))
}

fn render_with(text: &str, styles: MdStyles) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut in_code_block = false;
    let mut code_block_lines: Vec<String> = Vec::new();
    let mut code_lang = String::new();

    for raw_line in text.split('\n') {
        if raw_line.starts_with("```") {
            if in_code_block {
                lines.extend(render_code_block(&code_block_lines, &code_lang, styles));
                code_block_lines.clear();
                code_lang.clear();
                in_code_block = false;
            } else {
                in_code_block = true;
                code_lang = raw_line.trim_start_matches("```").trim().to_string();
            }
            continue;
        }

        if in_code_block {
            code_block_lines.push(raw_line.to_string());
            continue;
        }

        if raw_line.trim().is_empty() {
            lines.push(Line::from(""));
            continue;
        }

        if let Some(rendered) = render_heading(raw_line, styles) {
            lines.push(rendered);
            continue;
        }

        if let Some(rendered) = render_blockquote(raw_line, styles) {
            lines.push(rendered);
            continue;
        }

        if let Some(rendered) = render_list_item(raw_line, styles) {
            lines.push(rendered);
            continue;
        }

        lines.push(render_inline(raw_line, Style::default(), styles));
    }

    if in_code_block {
        lines.extend(render_code_block(&code_block_lines, &code_lang, styles));
    }

    lines
}

fn render_heading(line: &str, styles: MdStyles) -> Option<Line<'static>> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let mut after = trimmed[level..].trim_start();
    if after.starts_with(' ') {
        after = after.trim_start();
    }
    let style = match level {
        1 => styles.heading1,
        2 => styles.heading2,
        3 => styles.heading3,
        _ => styles.heading_rest,
    };
    let prefix = "#".repeat(level);
    Some(Line::from(vec![
        Span::styled(format!("{} ", prefix), style),
        Span::styled(after.to_string(), style),
    ]))
}

fn render_blockquote(line: &str, styles: MdStyles) -> Option<Line<'static>> {
    if !line.trim_start().starts_with('>') {
        return None;
    }
    let content = line
        .trim_start()
        .trim_start_matches('>')
        .trim_start()
        .to_string();
    Some(Line::from(vec![
        Span::styled("▎ ", styles.quote_bar),
        Span::styled(content, styles.quote_text),
    ]))
}

fn render_list_item(line: &str, styles: MdStyles) -> Option<Line<'static>> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        let content = &trimmed[2..];
        let mut spans: Vec<Span<'static>> = Vec::new();
        if indent > 0 {
            spans.push(Span::raw(" ".repeat(indent)));
        }
        spans.push(Span::styled("• ", styles.list_marker));
        spans.extend(parse_inline_spans(content, Style::default(), styles));
        return Some(Line::from(spans));
    }
    if let Some(rest) = trimmed.strip_prefix(|c: char| c.is_ascii_digit()) {
        if rest.starts_with(". ") {
            let num = &trimmed[..trimmed.find('.').unwrap_or(0)];
            let content = &rest[2..];
            let mut spans: Vec<Span<'static>> = Vec::new();
            if indent > 0 {
                spans.push(Span::raw(" ".repeat(indent)));
            }
            spans.push(Span::styled(format!("{num}. "), styles.list_marker));
            spans.extend(parse_inline_spans(content, Style::default(), styles));
            return Some(Line::from(spans));
        }
    }
    None
}

/// One ratatui `Line` per source line. Uses syntect when a language is known
/// (or guessable); falls back to monochrome green on highlight failure.
fn render_code_block(lines: &[String], lang: &str, styles: MdStyles) -> Vec<Line<'static>> {
    if lines.is_empty() {
        return vec![Line::from("")];
    }

    let mono = styles.code_mono;
    let code = lines.join("\n");
    let ss = syntax_set();
    let syntax = if lang.is_empty() {
        ss.find_syntax_plain_text()
    } else {
        ss.find_syntax_by_token(lang)
            .or_else(|| ss.find_syntax_by_extension(lang))
            .or_else(|| ss.find_syntax_by_name(lang))
            .unwrap_or_else(|| ss.find_syntax_plain_text())
    };

    let mut highlighter = HighlightLines::new(syntax, syntect_theme());
    let mut out = Vec::new();

    for line in LinesWithEndings::from(&code) {
        match highlighter.highlight_line(line, ss) {
            Ok(ranges) => {
                let mut spans = Vec::new();
                for (style, text) in ranges {
                    let text = text.trim_end_matches(['\r', '\n']);
                    if text.is_empty() {
                        continue;
                    }
                    spans.push(Span::styled(
                        text.to_string(),
                        syntect_style_to_ratatui(style),
                    ));
                }
                if spans.is_empty() {
                    out.push(Line::from(""));
                } else {
                    out.push(Line::from(spans));
                }
            }
            Err(_) => {
                out.push(Line::from(Span::styled(
                    line.trim_end_matches(['\r', '\n']).to_string(),
                    mono,
                )));
            }
        }
    }

    if out.is_empty() {
        out.push(Line::from(""));
    }
    out
}

fn syntect_style_to_ratatui(style: SynStyle) -> Style {
    let fg = style.foreground;
    let mut s = Style::new().fg(Color::Rgb(fg.r, fg.g, fg.b));
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::BOLD)
    {
        s = s.add_modifier(Modifier::BOLD);
    }
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::ITALIC)
    {
        s = s.add_modifier(Modifier::ITALIC);
    }
    if style
        .font_style
        .contains(syntect::highlighting::FontStyle::UNDERLINE)
    {
        s = s.add_modifier(Modifier::UNDERLINED);
    }
    s
}

fn render_inline(text: &str, base: Style, styles: MdStyles) -> Line<'static> {
    Line::from(parse_inline_spans(text, base, styles))
}

fn parse_inline_spans(text: &str, base: Style, styles: MdStyles) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut i = 0;
    let chars: Vec<char> = text.chars().collect();

    while i < chars.len() {
        if chars[i] == '`' {
            let start = i + 1;
            if let Some(end) = chars[start..].iter().position(|c| *c == '`') {
                let code: String = chars[start..start + end].iter().collect();
                spans.push(Span::styled(code, styles.inline_code));
                i = start + end + 1;
                continue;
            }
        }

        if chars[i] == '[' {
            let text_start = i + 1;
            if let Some(close_bracket) = chars[text_start..].iter().position(|c| *c == ']') {
                let link_text: String = chars[text_start..text_start + close_bracket]
                    .iter()
                    .collect();
                let after = text_start + close_bracket + 1;
                if after < chars.len() && chars[after] == '(' {
                    if let Some(close_paren) = chars[after + 1..].iter().position(|c| *c == ')') {
                        let _url: String =
                            chars[after + 1..after + 1 + close_paren].iter().collect();
                        spans.push(Span::styled(link_text, styles.link));
                        i = after + 1 + close_paren + 1;
                        continue;
                    }
                }
            }
        }

        if chars[i] == '*' && i + 1 < chars.len() && chars[i + 1] == '*' {
            let start = i + 2;
            if let Some(end) = find_closing(&chars, start, "**") {
                let inner: String = chars[start..end].iter().collect();
                let inner_spans =
                    parse_inline_spans_bold(&inner, base.add_modifier(Modifier::BOLD), styles);
                spans.extend(inner_spans);
                i = end + 2;
                continue;
            }
        }

        if chars[i] == '*' {
            let start = i + 1;
            if let Some(end) = find_closing(&chars, start, "*") {
                let inner: String = chars[start..end].iter().collect();
                spans.push(Span::styled(inner, base.add_modifier(Modifier::ITALIC)));
                i = end + 1;
                continue;
            }
        }

        spans.push(Span::styled(chars[i].to_string(), base));
        i += 1;
    }

    spans
}

fn parse_inline_spans_bold(text: &str, base: Style, styles: MdStyles) -> Vec<Span<'static>> {
    parse_inline_spans(text, base, styles)
}

fn find_closing(chars: &[char], start: usize, delim: &str) -> Option<usize> {
    let dc: Vec<char> = delim.chars().collect();
    let mut i = start;
    while i + dc.len() <= chars.len() {
        if chars[i..].starts_with(&dc) {
            return Some(i);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_theme() -> Theme {
        crate::theme::default_theme()
    }

    fn render_test(text: &str) -> Vec<Line<'static>> {
        render(text, &test_theme())
    }

    #[test]
    fn test_plain_text() {
        let lines = render_test("hello world");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].to_string().contains("hello world"));
    }

    #[test]
    fn test_empty_input() {
        let lines = render_test("");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].to_string(), "");
    }

    #[test]
    fn test_blank_line() {
        let lines = render_test("\n");
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_bold() {
        let lines = render_test("hello **world** here");
        assert_eq!(lines.len(), 1);
        let s = lines[0].to_string();
        assert!(s.contains("world"), "bold text should be present: {:?}", s);
    }

    #[test]
    fn test_italic() {
        let lines = render_test("hello *world* here");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_inline_code() {
        let lines = render_test("use `cargo build` to compile");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_inline_code_matches_theme_accent() {
        let theme = test_theme();
        let lines = render("- `src/main.rs` - entrypoint", &theme);
        assert_eq!(lines.len(), 1);
        let code_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "src/main.rs")
            .expect("inline code span for path");
        assert_eq!(
            code_span.style.fg, theme.accent_fg.fg,
            "file path inline code must use theme accent (prompt colour)"
        );
        let bullet = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "• ")
            .expect("bullet marker");
        assert_eq!(
            bullet.style.fg, theme.accent_fg.fg,
            "list marker should match prompt colour too"
        );
    }

    #[test]
    fn test_heading_h1() {
        let lines = render_test("# Title");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_heading_h2() {
        let lines = render_test("## Subtitle");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_heading_h3() {
        let lines = render_test("### Subsection");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_blockquote() {
        let lines = render_test("> quoted text");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_unordered_list() {
        let lines = render_test("- item one\n- item two");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_ordered_list() {
        let lines = render_test("1. first\n2. second");
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_code_block() {
        let lines = render_test("```\nlet x = 42;\n```");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].to_string().contains("let x = 42"));
    }

    #[test]
    fn test_code_block_with_lang() {
        let lines = render_test("```rust\nfn main() {\n    let x = 1;\n}\n```");
        assert_eq!(lines.len(), 3, "one Line per source line, got {lines:?}");
        let joined: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(joined.contains("fn main"), "got: {joined}");
        assert!(joined.contains("let x"), "got: {joined}");
        // At least one span should use a non-default RGB color from the theme
        // (syntect highlight for Rust keywords/identifiers).
        let has_rgb = lines.iter().any(|line| {
            line.spans
                .iter()
                .any(|sp| matches!(sp.style.fg, Some(Color::Rgb(_, _, _))))
        });
        assert!(has_rgb, "expected syntect RGB colors on rust fence");
    }

    #[test]
    fn test_code_block_unknown_lang_still_renders() {
        let lines = render_test("```not-a-real-lang\nfoo bar\n```");
        assert_eq!(lines.len(), 1);
        assert!(lines[0].to_string().contains("foo bar"));
    }

    #[test]
    fn test_link() {
        let lines = render_test("[text](url)");
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_unclosed_code_block_does_not_crash() {
        let lines = render_test("```\nunclosed");
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_multiline_paragraph() {
        let lines = render_test("line one\n\nline two");
        assert_eq!(lines.len(), 3);
    }
}
