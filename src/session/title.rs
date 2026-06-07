//! Session title formatting utilities

use regex::Regex;

/// Maximum length for a session title
pub const MAX_TITLE_LENGTH: usize = 100;

/// Format a session title from user input
pub fn format_title(input: &str) -> String {
    let trimmed = input.trim();

    // Remove newlines and excessive whitespace
    let cleaned = trimmed
        .lines()
        .next()
        .unwrap_or("")
        .replace('\t', " ");

    // Collapse multiple spaces
    let re = Regex::new(r"\s+").unwrap();
    let collapsed = re.replace_all(&cleaned, " ");

    // Truncate if too long
    if collapsed.len() > MAX_TITLE_LENGTH {
        format!("{}...", &collapsed[..MAX_TITLE_LENGTH - 3])
    } else {
        collapsed.to_string()
    }
}

/// Generate a default title from the first user message
pub fn generate_default_title(first_message: &str) -> String {
    let formatted = format_title(first_message);

    if formatted.is_empty() {
        "Untitled Session".to_string()
    } else {
        formatted
    }
}

/// Sanitize a title for use in filenames
pub fn sanitize_for_filename(title: &str) -> String {
    title
        .replace(|c: char| !c.is_alphanumeric() && c != ' ' && c != '-', "_")
        .replace(' ', "_")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_title() {
        assert_eq!(format_title("Hello World"), "Hello World");
        assert_eq!(format_title("  Hello   World  "), "Hello World");
        assert_eq!(format_title("Hello\nWorld"), "Hello");
        assert_eq!(format_title("Hello\tWorld"), "Hello World");
    }

    #[test]
    fn test_format_title_truncate() {
        let long = "a".repeat(150);
        let result = format_title(&long);
        assert_eq!(result.len(), MAX_TITLE_LENGTH);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_generate_default_title() {
        assert_eq!(generate_default_title("Hello World"), "Hello World");
        assert_eq!(generate_default_title(""), "Untitled Session");
        assert_eq!(generate_default_title("   "), "Untitled Session");
    }

    #[test]
    fn test_sanitize_for_filename() {
        assert_eq!(sanitize_for_filename("Hello World"), "hello_world");
        assert_eq!(sanitize_for_filename("Test-123"), "test-123");
        assert_eq!(sanitize_for_filename("A/B\\C"), "a_b_c");
    }
}
