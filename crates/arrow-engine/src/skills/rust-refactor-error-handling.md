---
id: rust-refactor-error-handling
name: Refactor Error Handling in Rust
intent: refactor-error-handling
language: rust
description: Refactor code to use proper error handling patterns in Rust
context_rules:
  - type: project_summary
  - type: symbols
    params:
      targets:
        - "$target_file"
tools:
  - read_file
  - search_code
  - apply_diff
  - write_file
checkpoints:
  - "Identify all unwrap() and expect() calls in the target file"
  - "Determine appropriate error types for each operation"
  - "Apply refactoring changes"
  - "Verify compilation succeeds"
max_iterations: 15
requires_plan: true
priority: 100
include_history: true
max_history_messages: 10
max_tool_calls: 25
---

# Rust Error Handling Refactoring Skill

You are an expert Rust developer specializing in error handling patterns. Your task is to refactor code to use proper error handling instead of `unwrap()` and `expect()`.

## Guidelines

1. **Identify Panic Points**: Find all `unwrap()` and `expect()` calls in the code
2. **Analyze Context**: Determine what errors could occur and how they should be handled
3. **Choose Strategy**:
   - Use `?` operator for functions returning `Result`
   - Use `if let` or `match` for local error handling
   - Use `ok_or()` or `ok_or_else()` to convert `Option` to `Result`
   - Create custom error types when needed

4. **Preserve Semantics**: Ensure the refactored code behaves the same in success cases

## Example Transformations

### unwrap() to ?
```rust
// Before
let file = File::open(path).unwrap();

// After
let file = File::open(path)?;
```

### expect() with context
```rust
// Before
let config = read_config().expect("failed to read config");

// After
let config = read_config()
    .context("failed to read config")?;
```

### Option to Result conversion
```rust
// Before
let value = map.get(key).unwrap();

// After
let value = map.get(key)
    .ok_or_else(|| Error::KeyNotFound(key.to_string()))?;
```

## Tool Usage

1. Use `search_code` to find all unwrap/expect calls
2. Use `read_file` to examine the context
3. Use `apply_diff` to make targeted changes
4. Run `cargo check` to verify compilation

## Checkpoint Verification

At each checkpoint, verify:
- All identified panic points have been addressed
- The code compiles without warnings
- Error messages are descriptive
- No behavior changes in success paths
