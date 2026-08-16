//! Session ID generation with a stable suffix preserved across compact/fork/rewind

use rand::RngExt;

/// Generate a UUID-shaped session ID with an optional stable suffix
///
/// The last segment (12 hex chars after the final hyphen) is either
/// the provided *suffix* or freshly random. The first four segments
/// (20 hex chars) are always random.
///
/// Format: `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`
pub fn generate_session_id(suffix: Option<&str>) -> String {
    let mut rng = rand::rng();
    let head: Vec<u8> = (0..10).map(|_| rng.random()).collect(); // 10 bytes = 20 hex chars
    let tail = suffix.map(|s| s.to_string()).unwrap_or_else(|| {
        let tail_bytes: Vec<u8> = (0..6).map(|_| rng.random()).collect(); // 6 bytes = 12 hex chars
        hex::encode(&tail_bytes)
    });

    let head_hex = hex::encode(&head);
    format!(
        "{}-{}-{}-{}-{}",
        &head_hex[0..8],
        &head_hex[8..12],
        &head_hex[12..16],
        &head_hex[16..20],
        tail
    )
}

/// Extract the stable suffix (last segment after the final hyphen)
pub fn extract_suffix(session_id: &str) -> String {
    session_id
        .rsplitn(2, '-')
        .next()
        .unwrap_or(session_id)
        .to_string()
}

/// Return a short human-readable slice of a session ID (8 chars)
pub fn shorten_session_id(session_id: &str, from_end: bool) -> String {
    const SHORT_LEN: usize = 8;
    if from_end {
        session_id.chars().rev().take(SHORT_LEN).collect::<String>().chars().rev().collect()
    } else {
        session_id.chars().take(SHORT_LEN).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_session_id_format() {
        let id = generate_session_id(None);
        let parts: Vec<&str> = id.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(parts[0].len(), 8);
        assert_eq!(parts[1].len(), 4);
        assert_eq!(parts[2].len(), 4);
        assert_eq!(parts[3].len(), 4);
        assert_eq!(parts[4].len(), 12);
    }

    #[test]
    fn test_generate_session_id_with_suffix() {
        let id = generate_session_id(Some("abc123"));
        assert!(id.ends_with("abc123"));
    }

    #[test]
    fn test_extract_suffix() {
        let id = "12345678-1234-1234-1234-abcdef123456";
        assert_eq!(extract_suffix(id), "abcdef123456");
    }

    #[test]
    fn test_shorten_session_id() {
        let id = "12345678-1234-1234-1234-abcdef123456";
        assert_eq!(shorten_session_id(id, false), "12345678");
        assert_eq!(shorten_session_id(id, true), "ef123456");
    }
}
