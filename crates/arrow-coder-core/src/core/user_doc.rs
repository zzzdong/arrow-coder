//! Structured user-document model.
//!
//! The composer (VS Code webview, CLI) builds a [`UserDoc`] from ordered
//! [`DocBlock`]s instead of flattening `@path` references into a single string.
//! This preserves the relative position of plain text and references, and lets
//! references carry structured metadata (line ranges, reference kind) so the
//! core can expand them precisely.
//!
//! For backwards compatibility, [`UserDoc::from_text`] re-parses a plain
//! message that still uses the `@path` convention (used by the CLI and legacy
//! hosts), producing a single text block plus trailing [`DocBlock::FileRef`]s.

use serde::{Deserialize, Serialize};

/// A 1-based inclusive line range within a referenced file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefRange {
    /// 1-based start line (inclusive).
    pub start: usize,
    /// 1-based end line (inclusive).
    pub end: usize,
}

impl RefRange {
    /// Extrct a subset of `lines` (0-based internally) covered by this range.
    pub fn slice<'a>(&self, lines: &'a [String]) -> &'a [String] {
        let lo = self.start.saturating_sub(1);
        let hi = (self.end).min(lines.len());
        if lo >= hi {
            return &lines[..0];
        }
        &lines[lo..hi]
    }

    /// Whether this range is valid (start <= end, both >= 1).
    pub fn is_valid(&self) -> bool {
        self.start >= 1 && self.end >= self.start
    }
}

/// The semantic kind of a reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// A whole file (read in full).
    File,
    /// A whole directory (read recursively up to a depth limit).
    Dir,
    /// An editor selection — a range of lines, optionally with the snippet text
    /// captured at reference time (used for display + fallback if the file
    /// changed before expansion).
    Selection,
    /// A binary / image attachment (not inlined as text; passed as an image
    /// attachment to multimodal backends).
    Image,
}

/// A single block of a [`UserDoc`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocBlock {
    /// Free-form plain text written by the user.
    Text { text: String },
    /// A reference to a file, directory, selection, or image located on disk.
    Ref {
        /// Reference kind (file / dir / selection / image).
        kind: RefKind,
        /// Filesystem path (absolute or relative to the working dir).
        path: String,
        /// For `Selection` refs: the 1-based inclusive line range.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        range: Option<RefRange>,
        /// For `Selection` refs: the snippet captured at reference time.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        snippet: Option<String>,
        /// For `Dir` refs: recursion depth limit (1 = direct children only).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        depth: Option<u8>,
    },
}

impl DocBlock {
    /// Convenience constructor for a text block.
    pub fn text(s: impl Into<String>) -> Self {
        DocBlock::Text { text: s.into() }
    }

    /// Convenience constructor for a whole-file reference.
    pub fn file_ref(path: impl Into<String>) -> Self {
        DocBlock::Ref {
            kind: RefKind::File,
            path: path.into(),
            range: None,
            snippet: None,
            depth: None,
        }
    }

    /// Convenience constructor for a directory reference.
    pub fn dir_ref(path: impl Into<String>, depth: Option<u8>) -> Self {
        DocBlock::Ref {
            kind: RefKind::Dir,
            path: path.into(),
            range: None,
            snippet: None,
            depth,
        }
    }

    /// Convenience constructor for a selection reference.
    pub fn selection_ref(
        path: impl Into<String>,
        range: RefRange,
        snippet: Option<String>,
    ) -> Self {
        DocBlock::Ref {
            kind: RefKind::Selection,
            path: path.into(),
            range: Some(range),
            snippet,
            depth: None,
        }
    }

    /// Convenience constructor for an image reference.
    pub fn image_ref(path: impl Into<String>) -> Self {
        DocBlock::Ref {
            kind: RefKind::Image,
            path: path.into(),
            range: None,
            snippet: None,
            depth: None,
        }
    }

    /// Whether this block is a reference (vs plain text).
    pub fn is_ref(&self) -> bool {
        matches!(self, DocBlock::Ref { .. })
    }
}

/// An ordered, structured user document.
///
/// The order of `blocks` is meaningful: the core expands references *in place*,
/// preserving the relative position of text and references as authored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct UserDoc {
    #[serde(default)]
    pub blocks: Vec<DocBlock>,
}

impl UserDoc {
    /// An empty document.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a document from a list of blocks.
    pub fn from_blocks(blocks: Vec<DocBlock>) -> Self {
        Self { blocks }
    }

    /// Whether the document is empty (no blocks, or all blocks blank).
    pub fn is_empty(&self) -> bool {
        if self.blocks.is_empty() {
            return true;
        }
        self.blocks.iter().all(|b| match b {
            DocBlock::Text { text } => text.trim().is_empty(),
            DocBlock::Ref { .. } => false,
        })
    }

    /// All references in document order.
    pub fn refs(&self) -> impl Iterator<Item = &DocBlock> {
        self.blocks.iter().filter(|b| b.is_ref())
    }

    /// Parse a legacy plain-text message containing `@path` references.
    ///
    /// The text (with `@path` tokens stripped) becomes a single leading
    /// [`DocBlock::Text`]; every referenced path becomes a trailing
    /// [`DocBlock::FileRef`] in order of appearance. This keeps the CLI and any
    /// host that still uses the `@path` convention working unchanged.
    pub fn from_text(content: &str, references: &[String]) -> Self {
        let mut blocks = Vec::new();
        // Strip `@path` tokens from the inline text to avoid double expansion.
        let cleaned = if references.is_empty() {
            content.to_string()
        } else {
            let mut s = content.to_string();
            for r in references {
                s = s.replace(&format!("@{}", r), "").replace("@/", "");
            }
            s
        };
        let cleaned = cleaned.trim_matches('\n').to_string();
        if !cleaned.is_empty() {
            blocks.push(DocBlock::text(cleaned));
        }
        for r in references {
            blocks.push(DocBlock::file_ref(r.clone()));
        }
        Self { blocks }
    }
}
