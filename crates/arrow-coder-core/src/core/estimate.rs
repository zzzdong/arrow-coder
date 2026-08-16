//! Lightweight, dependency-free token estimation.
//!
//! This mirrors deepseek-harness's `token-meter` approach: context capacity is
//! projected from the *current surface* (system + tools + messages) using a
//! cheap heuristic rather than a precise BPE tokenizer. The harness source
//! notes these character-based estimates are intentionally rough and are
//! *anchored* to the last real provider-reported prompt size so the absolute
//! scale stays accurate while the deltas (compaction, new turns) react
//! immediately.
//!
//! Density heuristic (tokens per run of characters):
//! - CJK ideographs compress well in real tokenizers, so they are *under*-
//!   estimated here (~1.5 chars/token) — matching the harness comment that
//!   these estimates run low for CJK/JSON.
//! - Everything else (ASCII, JSON, code) is estimated at ~4 chars/token.

/// Estimate the token count of a piece of text using a character-density
/// heuristic. Returns a saturating lower-bound-ish estimate (harness-style).
pub fn estimate_tokens(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let chars: u64 = text.chars().count() as u64;
    // Count CJK codepoints; they are denser (fewer chars per token).
    let cjk: u64 = text
        .chars()
        .filter(|c| {
            let cp = *c as u32;
            // CJK Unified Ideographs + common extensions + Japanese kana.
            (0x4E00..=0x9FFF).contains(&cp)
                || (0x3400..=0x4DBF).contains(&cp)
                || (0x3040..=0x30FF).contains(&cp)
                || (0xAC00..=0xD7AF).contains(&cp)
        })
        .count() as u64;
    let other = chars.saturating_sub(cjk);
    // CJK ~1.5 chars/token (under-estimate); other ~4 chars/token.
    let cjk_tokens = cjk.div_ceil(2).max(cjk / 2); // ~1.5/chars ⇒ ceil(x/1.5)
    let other_tokens = other / 4;
    cjk_tokens + other_tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn ascii_roughly_four_per_token() {
        let s = "fn main() { let x = 1; }"; // 23 chars
        let t = estimate_tokens(s);
        assert!(t >= 5 && t <= 8, "got {t}");
    }

    #[test]
    fn cjk_is_denser() {
        let s = "我们需要修复这个上下文压缩的问题"; // 16 CJK chars
        let t = estimate_tokens(s);
        // ~1.5 chars/token ⇒ ~10-11 tokens
        assert!(t >= 8 && t <= 12, "got {t}");
    }
}
