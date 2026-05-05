//! Symbol extraction using tree-sitter
//!
//! This module provides tree-sitter based symbol extraction for multiple languages,
//! replacing the previous regex-based approach with accurate AST parsing.

use crate::project::types::*;
use std::collections::HashMap;
use std::path::Path;
use tree_sitter::{Language, Node, Parser, Tree};

/// Symbol extractor trait
pub trait SymbolExtractor: Send + Sync {
    /// Extract symbols from file content
    fn extract(
        &self,
        file_path: &Path,
        content: &str,
        language: &str,
    ) -> anyhow::Result<FileSymbols>;
}

/// Tree-sitter based symbol extractor
pub struct TreeSitterExtractor {
    /// Language parsers
    parsers: HashMap<String, Language>,
}

impl TreeSitterExtractor {
    /// Create a new tree-sitter extractor with all supported languages
    pub fn new() -> Self {
        let mut parsers = HashMap::new();

        // Register supported languages
        parsers.insert("rust".to_string(), tree_sitter_rust::LANGUAGE.into());
        parsers.insert("python".to_string(), tree_sitter_python::LANGUAGE.into());
        parsers.insert("javascript".to_string(), tree_sitter_javascript::LANGUAGE.into());
        parsers.insert("typescript".to_string(), tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into());
        parsers.insert("tsx".to_string(), tree_sitter_typescript::LANGUAGE_TSX.into());

        Self { parsers }
    }

    /// Check if language is supported
    pub fn supports_language(&self, language: &str) -> bool {
        self.parsers.contains_key(language)
    }

    /// Get parser for language
    fn get_parser(&self, language: &str) -> anyhow::Result<Parser> {
        let lang = self
            .parsers
            .get(language)
            .ok_or_else(|| anyhow::anyhow!("Unsupported language: {}", language))?;

        let mut parser = Parser::new();
        parser.set_language(lang)?;
        Ok(parser)
    }

    /// Calculate content hash
    fn calculate_hash(content: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

impl Default for TreeSitterExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolExtractor for TreeSitterExtractor {
    fn extract(
        &self,
        file_path: &Path,
        content: &str,
        language: &str,
    ) -> anyhow::Result<FileSymbols> {
        let content_hash = Self::calculate_hash(content);

        // Parse the file
        let mut parser = self.get_parser(language)?;
        let tree = parser
            .parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse file: {}", file_path.display()))?;

        // Extract symbols based on language
        let (symbols, imports, exports) = match language {
            "rust" => extract_rust_symbols(&tree, content, file_path)?,
            "python" => extract_python_symbols(&tree, content, file_path)?,
            "javascript" | "typescript" | "tsx" => {
                extract_js_ts_symbols(&tree, content, file_path, language)?
            }
            _ => (vec![], vec![], vec![]),
        };

        Ok(FileSymbols {
            file_path: file_path.to_string_lossy().to_string(),
            language: language.to_string(),
            symbols,
            imports,
            exports,
            content_hash,
        })
    }
}

/// Extract Rust symbols using tree-sitter
fn extract_rust_symbols(
    tree: &Tree,
    content: &str,
    file_path: &Path,
) -> anyhow::Result<(Vec<Symbol>, Vec<String>, Vec<String>)> {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut exports = Vec::new();

    let root = tree.root_node();
    let mut cursor = root.walk();

    // Walk the tree
    for node in root.children(&mut cursor) {
        match node.kind() {
            // Module-level items
            "function_item" | "function_signature_item" => {
                if let Some(symbol) = extract_rust_function(&node, content, file_path, None) {
                    if symbol.visibility == "pub" || symbol.visibility == "public" {
                        exports.push(symbol.name.clone());
                    }
                    symbols.push(symbol);
                }
            }
            "struct_item" => {
                if let Some(symbol) = extract_rust_struct(&node, content, file_path) {
                    if symbol.visibility == "pub" || symbol.visibility == "public" {
                        exports.push(symbol.name.clone());
                    }
                    symbols.push(symbol);
                }
            }
            "enum_item" => {
                if let Some(symbol) = extract_rust_enum(&node, content, file_path) {
                    if symbol.visibility == "pub" || symbol.visibility == "public" {
                        exports.push(symbol.name.clone());
                    }
                    symbols.push(symbol);
                }
            }
            "trait_item" => {
                if let Some(symbol) = extract_rust_trait(&node, content, file_path) {
                    if symbol.visibility == "pub" || symbol.visibility == "public" {
                        exports.push(symbol.name.clone());
                    }
                    symbols.push(symbol);
                }
            }
            "impl_item" => {
                if let Some(impl_symbols) = extract_rust_impl(&node, content, file_path) {
                    for symbol in &impl_symbols {
                        if symbol.visibility == "pub" || symbol.visibility == "public" {
                            exports.push(symbol.name.clone());
                        }
                    }
                    symbols.extend(impl_symbols);
                }
            }
            "type_item" => {
                if let Some(symbol) = extract_rust_type_alias(&node, content, file_path) {
                    if symbol.visibility == "pub" || symbol.visibility == "public" {
                        exports.push(symbol.name.clone());
                    }
                    symbols.push(symbol);
                }
            }
            "const_item" | "static_item" => {
                if let Some(symbol) = extract_rust_const_static(&node, content, file_path) {
                    if symbol.visibility == "pub" || symbol.visibility == "public" {
                        exports.push(symbol.name.clone());
                    }
                    symbols.push(symbol);
                }
            }
            "macro_definition" => {
                if let Some(symbol) = extract_rust_macro(&node, content, file_path) {
                    if symbol.visibility == "pub" || symbol.visibility == "public" {
                        exports.push(symbol.name.clone());
                    }
                    symbols.push(symbol);
                }
            }
            "use_declaration" => {
                let import = extract_rust_import(&node, content);
                imports.push(import);
            }
            "mod_item" => {
                if let Some(symbol) = extract_rust_module(&node, content, file_path) {
                    if symbol.visibility == "pub" || symbol.visibility == "public" {
                        exports.push(symbol.name.clone());
                    }
                    symbols.push(symbol);
                }
            }
            _ => {}
        }
    }

    Ok((symbols, imports, exports))
}

/// Extract Rust function
fn extract_rust_function(
    node: &Node,
    content: &str,
    file_path: &Path,
    parent: Option<String>,
) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    // Get visibility
    let visibility = get_rust_visibility(node, content);

    // Get function name
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(content.as_bytes()).ok()?.to_string();

    // Get signature
    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);

    // Get doc comment
    let documentation = extract_rust_doc_comment(node, content);

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Function,
        full_name: if let Some(ref p) = parent {
            format!("{}::{}", p, name)
        } else {
            name.clone()
        },
        documentation,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility,
        parent,
        children: vec![],
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Rust struct
fn extract_rust_struct(node: &Node, content: &str, file_path: &Path) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let visibility = get_rust_visibility(node, content);
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(content.as_bytes()).ok()?.to_string();

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);
    let documentation = extract_rust_doc_comment(node, content);

    // Extract struct fields as children
    let mut children = vec![];
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for field in body.children(&mut cursor) {
            if field.kind() == "field_declaration" {
                if let Some(field_symbol) = extract_rust_field(&field, content, file_path, &name) {
                    children.push(field_symbol);
                }
            }
        }
    }

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Struct,
        full_name: name.clone(),
        documentation,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility,
        parent: None,
        children,
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Rust enum
fn extract_rust_enum(node: &Node, content: &str, file_path: &Path) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let visibility = get_rust_visibility(node, content);
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(content.as_bytes()).ok()?.to_string();

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);
    let documentation = extract_rust_doc_comment(node, content);

    // Extract enum variants as children
    let mut children = vec![];
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for variant in body.children(&mut cursor) {
            if variant.kind() == "enum_variant" {
                if let Some(variant_symbol) =
                    extract_rust_enum_variant(&variant, content, file_path, &name)
                {
                    children.push(variant_symbol);
                }
            }
        }
    }

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Enum,
        full_name: name.clone(),
        documentation,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility,
        parent: None,
        children,
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Rust trait
fn extract_rust_trait(node: &Node, content: &str, file_path: &Path) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let visibility = get_rust_visibility(node, content);
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(content.as_bytes()).ok()?.to_string();

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);
    let documentation = extract_rust_doc_comment(node, content);

    // Extract trait items as children
    let mut children = vec![];
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for item in body.children(&mut cursor) {
            match item.kind() {
                "function_signature_item" => {
                    if let Some(method) =
                        extract_rust_function(&item, content, file_path, Some(name.clone()))
                    {
                        children.push(method);
                    }
                }
                "const_item" => {
                    if let Some(const_item) =
                        extract_rust_const_static(&item, content, file_path)
                    {
                        children.push(const_item);
                    }
                }
                "type_item" => {
                    if let Some(type_item) = extract_rust_type_alias(&item, content, file_path) {
                        children.push(type_item);
                    }
                }
                _ => {}
            }
        }
    }

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Trait,
        full_name: name.clone(),
        documentation,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility,
        parent: None,
        children,
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Rust impl block
fn extract_rust_impl(
    node: &Node,
    content: &str,
    file_path: &Path,
) -> Option<Vec<Symbol>> {
    let start_pos = node.start_position();

    // Get the type being implemented
    let type_node = node.child_by_field_name("type")?;
    let type_name = type_node.utf8_text(content.as_bytes()).ok()?.to_string();

    let mut symbols = vec![];

    // Extract impl items
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for item in body.children(&mut cursor) {
            match item.kind() {
                "function_item" => {
                    if let Some(method) =
                        extract_rust_function(&item, content, file_path, Some(type_name.clone()))
                    {
                        symbols.push(method);
                    }
                }
                "const_item" => {
                    if let Some(const_item) = extract_rust_const_static(&item, content, file_path) {
                        let mut const_item = const_item;
                        const_item.parent = Some(type_name.clone());
                        symbols.push(const_item);
                    }
                }
                _ => {}
            }
        }
    }

    // Also add the impl block itself as a symbol
    let impl_symbol = Symbol {
        name: format!("impl {}", type_name),
        kind: SymbolKind::Impl,
        full_name: type_name.clone(),
        documentation: None,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: start_pos.row as u32 + 1,
            end_column: start_pos.column as u32 + 1,
        },
        visibility: "public".to_string(),
        parent: None,
        children: symbols.clone(),
        signature: Some(format!("impl {}", type_name)),
        type_info: None,
    };
    symbols.push(impl_symbol);

    Some(symbols)
}

/// Extract Rust field
fn extract_rust_field(
    node: &Node,
    content: &str,
    file_path: &Path,
    parent: &str,
) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let visibility = get_rust_visibility(node, content);

    // Get field name
    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(content.as_bytes()).ok())
        .map(|s| s.to_string())?;

    // Get field type
    let type_info = node
        .child_by_field_name("type")
        .and_then(|t| t.utf8_text(content.as_bytes()).ok())
        .map(|s| s.to_string());

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Field,
        full_name: format!("{}::{}", parent, name),
        documentation: None,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility,
        parent: Some(parent.to_string()),
        children: vec![],
        signature: Some(signature),
        type_info,
    })
}

/// Extract Rust enum variant
fn extract_rust_enum_variant(
    node: &Node,
    content: &str,
    file_path: &Path,
    parent: &str,
) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(content.as_bytes()).ok()?.to_string();

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Variant,
        full_name: format!("{}::{}", parent, name),
        documentation: None,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility: "public".to_string(),
        parent: Some(parent.to_string()),
        children: vec![],
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Rust type alias
fn extract_rust_type_alias(node: &Node, content: &str, file_path: &Path) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let visibility = get_rust_visibility(node, content);
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(content.as_bytes()).ok()?.to_string();

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);
    let documentation = extract_rust_doc_comment(node, content);

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::TypeAlias,
        full_name: name.clone(),
        documentation,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility,
        parent: None,
        children: vec![],
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Rust const/static
fn extract_rust_const_static(node: &Node, content: &str, file_path: &Path) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let visibility = get_rust_visibility(node, content);
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(content.as_bytes()).ok()?.to_string();

    let kind = if node.kind() == "const_item" {
        SymbolKind::Const
    } else {
        SymbolKind::Static
    };

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);
    let documentation = extract_rust_doc_comment(node, content);

    Some(Symbol {
        name: name.clone(),
        kind,
        full_name: name.clone(),
        documentation,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility,
        parent: None,
        children: vec![],
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Rust macro
fn extract_rust_macro(node: &Node, content: &str, file_path: &Path) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let visibility = get_rust_visibility(node, content);

    // Get macro name (after macro_rules!)
    let name = node
        .child(1)
        .and_then(|n| n.utf8_text(content.as_bytes()).ok())
        .map(|s| s.to_string())?;

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);
    let documentation = extract_rust_doc_comment(node, content);

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Macro,
        full_name: name.clone(),
        documentation,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility,
        parent: None,
        children: vec![],
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Rust module
fn extract_rust_module(node: &Node, content: &str, file_path: &Path) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let visibility = get_rust_visibility(node, content);
    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(content.as_bytes()).ok()?.to_string();

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);
    let documentation = extract_rust_doc_comment(node, content);

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Module,
        full_name: name.clone(),
        documentation,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility,
        parent: None,
        children: vec![],
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Rust import
fn extract_rust_import(node: &Node, content: &str) -> String {
    node.utf8_text(content.as_bytes())
        .map(|s| s.to_string())
        .unwrap_or_default()
}

/// Get Rust visibility
fn get_rust_visibility(node: &Node, content: &str) -> String {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "visibility_modifier" => {
                let vis_text = child
                    .utf8_text(content.as_bytes())
                    .unwrap_or("pub")
                    .to_string();
                // Handle pub(crate), pub(super), etc.
                return if vis_text.starts_with("pub(") {
                    vis_text
                } else {
                    "pub".to_string()
                };
            }
            _ => {}
        }
    }
    "private".to_string()
}

/// Extract Rust doc comment
fn extract_rust_doc_comment(node: &Node, content: &str) -> Option<String> {
    let mut docs = vec![];

    // Look for outer attributes before this node
    let parent = node.parent()?;
    let mut cursor = parent.walk();

    for child in parent.children(&mut cursor) {
        if child == *node {
            break;
        }
        if child.kind() == "attribute_item" || child.kind() == "outer_attribute_comment" {
            let attr_text = child.utf8_text(content.as_bytes()).unwrap_or("");
            if attr_text.starts_with("///") || attr_text.starts_with("//!") {
                docs.push(attr_text.to_string());
            }
        }
    }

    if docs.is_empty() {
        None
    } else {
        Some(docs.join("\n"))
    }
}

/// Truncate signature for storage
fn truncate_signature(signature: &str) -> String {
    const MAX_LEN: usize = 500;
    if signature.len() > MAX_LEN {
        format!("{}...", &signature[..MAX_LEN])
    } else {
        signature.to_string()
    }
}

/// Extract Python symbols
fn extract_python_symbols(
    tree: &Tree,
    content: &str,
    file_path: &Path,
) -> anyhow::Result<(Vec<Symbol>, Vec<String>, Vec<String>)> {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut exports = Vec::new();

    let root = tree.root_node();
    let mut cursor = root.walk();

    for node in root.children(&mut cursor) {
        match node.kind() {
            "function_definition" => {
                if let Some(symbol) = extract_python_function(&node, content, file_path, None) {
                    exports.push(symbol.name.clone());
                    symbols.push(symbol);
                }
            }
            "class_definition" => {
                if let Some(symbol) = extract_python_class(&node, content, file_path) {
                    exports.push(symbol.name.clone());
                    symbols.push(symbol);
                }
            }
            "import_statement" | "import_from_statement" => {
                let import = node.utf8_text(content.as_bytes()).unwrap_or("").to_string();
                imports.push(import);
            }
            _ => {}
        }
    }

    Ok((symbols, imports, exports))
}

/// Extract Python function
fn extract_python_function(
    node: &Node,
    content: &str,
    file_path: &Path,
    parent: Option<String>,
) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(content.as_bytes()).ok()?.to_string();

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);

    let documentation = extract_python_docstring(node, content);

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Function,
        full_name: if let Some(ref p) = parent {
            format!("{}.{}", p, name)
        } else {
            name.clone()
        },
        documentation,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility: "public".to_string(),
        parent,
        children: vec![],
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Python class
fn extract_python_class(node: &Node, content: &str, file_path: &Path) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let name_node = node.child_by_field_name("name")?;
    let name = name_node.utf8_text(content.as_bytes()).ok()?.to_string();

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);
    let documentation = extract_python_docstring(node, content);

    // Extract class methods
    let mut children = vec![];
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for item in body.children(&mut cursor) {
            if item.kind() == "function_definition" {
                if let Some(method) =
                    extract_python_function(&item, content, file_path, Some(name.clone()))
                {
                    children.push(method);
                }
            }
        }
    }

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Class,
        full_name: name.clone(),
        documentation,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility: "public".to_string(),
        parent: None,
        children,
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract Python docstring
fn extract_python_docstring(node: &Node, content: &str) -> Option<String> {
    // Look for first string expression in body
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for child in body.children(&mut cursor) {
            if child.kind() == "expression_statement" {
                if let Some(expr) = child.child(0) {
                    if expr.kind() == "string" {
                        return expr
                            .utf8_text(content.as_bytes())
                            .ok()
                            .map(|s| s.to_string());
                    }
                }
            }
            // Only check first few children
            break;
        }
    }
    None
}

/// Extract JavaScript/TypeScript symbols
fn extract_js_ts_symbols(
    tree: &Tree,
    content: &str,
    file_path: &Path,
    _language: &str,
) -> anyhow::Result<(Vec<Symbol>, Vec<String>, Vec<String>)> {
    let mut symbols = Vec::new();
    let mut imports = Vec::new();
    let mut exports = Vec::new();

    let root = tree.root_node();
    let mut cursor = root.walk();

    for node in root.children(&mut cursor) {
        match node.kind() {
            "function_declaration" | "function" => {
                if let Some(symbol) = extract_js_function(&node, content, file_path, None) {
                    exports.push(symbol.name.clone());
                    symbols.push(symbol);
                }
            }
            "class_declaration" | "class" => {
                if let Some(symbol) = extract_js_class(&node, content, file_path) {
                    exports.push(symbol.name.clone());
                    symbols.push(symbol);
                }
            }
            "lexical_declaration" | "variable_declaration" => {
                // Check for exported const/let/var
                if let Some(symbol) = extract_js_variable(&node, content, file_path) {
                    exports.push(symbol.name.clone());
                    symbols.push(symbol);
                }
            }
            "import_statement" => {
                let import = node.utf8_text(content.as_bytes()).unwrap_or("").to_string();
                imports.push(import);
            }
            "export_statement" => {
                // Handle export { ... } or export const ...
                let export = node.utf8_text(content.as_bytes()).unwrap_or("").to_string();
                exports.push(export);
            }
            _ => {}
        }
    }

    Ok((symbols, imports, exports))
}

/// Extract JS/TS function
fn extract_js_function(
    node: &Node,
    content: &str,
    file_path: &Path,
    parent: Option<String>,
) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(content.as_bytes()).ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "anonymous".to_string());

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Function,
        full_name: if let Some(ref p) = parent {
            format!("{}.{}", p, name)
        } else {
            name.clone()
        },
        documentation: None,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility: "public".to_string(),
        parent,
        children: vec![],
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract JS/TS class
fn extract_js_class(node: &Node, content: &str, file_path: &Path) -> Option<Symbol> {
    let start_pos = node.start_position();
    let end_pos = node.end_position();

    let name = node
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(content.as_bytes()).ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "anonymous".to_string());

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);

    // Extract class methods
    let mut children = vec![];
    if let Some(body) = node.child_by_field_name("body") {
        let mut cursor = body.walk();
        for item in body.children(&mut cursor) {
            if item.kind() == "method_definition" || item.kind() == "function" {
                if let Some(method) =
                    extract_js_function(&item, content, file_path, Some(name.clone()))
                {
                    children.push(method);
                }
            }
        }
    }

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Class,
        full_name: name.clone(),
        documentation: None,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: end_pos.row as u32 + 1,
            end_column: end_pos.column as u32 + 1,
        },
        visibility: "public".to_string(),
        parent: None,
        children,
        signature: Some(signature),
        type_info: None,
    })
}

/// Extract JS/TS variable
fn extract_js_variable(node: &Node, content: &str, file_path: &Path) -> Option<Symbol> {
    let start_pos = node.start_position();

    // Get declarator
    let declarator = node.child_by_field_name("declarator")?;
    let name = declarator
        .child_by_field_name("name")
        .and_then(|n| n.utf8_text(content.as_bytes()).ok())
        .map(|s| s.to_string())?;

    let signature = node.utf8_text(content.as_bytes()).ok()?.to_string();
    let signature = truncate_signature(&signature);

    Some(Symbol {
        name: name.clone(),
        kind: SymbolKind::Variable,
        full_name: name.clone(),
        documentation: None,
        location: SymbolLocation {
            file_path: file_path.to_string_lossy().to_string(),
            start_line: start_pos.row as u32 + 1,
            start_column: start_pos.column as u32 + 1,
            end_line: start_pos.row as u32 + 1,
            end_column: start_pos.column as u32 + 1,
        },
        visibility: "public".to_string(),
        parent: None,
        children: vec![],
        signature: Some(signature),
        type_info: None,
    })
}

/// Simple symbol extractor (fallback)
pub struct SimpleSymbolExtractor;

impl SimpleSymbolExtractor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SimpleSymbolExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolExtractor for SimpleSymbolExtractor {
    fn extract(
        &self,
        file_path: &Path,
        content: &str,
        language: &str,
    ) -> anyhow::Result<FileSymbols> {
        // Fallback to basic extraction
        let content_hash = TreeSitterExtractor::calculate_hash(content);

        Ok(FileSymbols {
            file_path: file_path.to_string_lossy().to_string(),
            language: language.to_string(),
            symbols: vec![],
            imports: vec![],
            exports: vec![],
            content_hash,
        })
    }
}
