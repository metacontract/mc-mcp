// NOTE: use statements will need adjustment after moving files
use comrak::{markdown_to_html, ComrakOptions};

/// Parses a Markdown string and returns its plain text representation.
///
/// This function converts the Markdown to HTML first, then extracts the text content.
/// It uses default Comrak options.
///
/// # Arguments
///
/// * `markdown` - A string slice containing the Markdown text.
///
/// # Returns
///
/// A String containing the plain text extracted from the Markdown.
pub fn parse_markdown_to_text(markdown: &str) -> String {
    // TODO: より効率的なテキスト抽出方法を検討 (HTMLを経由しない方法)
    let html = markdown_to_html(markdown, &ComrakOptions::default());

    // HTML からテキストを抽出 (簡易的な方法)
    // より堅牢なライブラリ (例: scraper) の使用も検討できる
    html_to_text(&html)
}

// HTML文字列からタグを除去してテキストを抽出するヘルパー関数 (簡易版)
fn html_to_text(html: &str) -> String {
    let mut result = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => result.push(c),
            _ => {}
        }
    }
    // 簡単な空白の整形
    result.split_whitespace().collect::<Vec<&str>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_markdown() {
        let markdown = "# Header\n\nThis is **bold** text.";
        let expected_text = "Header This is bold text.";
        assert_eq!(parse_markdown_to_text(markdown), expected_text);
    }

    #[test]
    fn test_parse_markdown_with_link() {
        let markdown = "Visit [Google](https://google.com)!";
        let expected_text = "Visit Google!"; // Simple html_to_text extracts link text
        assert_eq!(parse_markdown_to_text(markdown), expected_text);
    }

    #[test]
    fn test_parse_markdown_with_list() {
        let markdown = "* Item 1\n* Item 2";
        // The current simple html_to_text might produce "Item 1 Item 2" or similar depending on HTML output
        let expected_text = "Item 1 Item 2";
        assert_eq!(parse_markdown_to_text(markdown), expected_text);
    }
}
