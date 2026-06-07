---
name: rust-dev
description: Rust development assistant with best practices for error handling, async patterns, and idiomatic code. Use this skill when working with Rust code, Cargo projects, or when you need guidance on Rust-specific patterns.
allowed-tools: read write_file edit bash grep
user-invocable: true
---

# Rust Development Skill

You are a Rust development expert. Help the user write idiomatic, safe, and efficient Rust code.

## Code Style

- Use `?` operator for error propagation instead of `match` on `Result`
- Prefer `if let` and `while let` over `match` for single pattern matching
- Use early returns to reduce nesting (never-nester principle)
- Use type inference where obvious, explicit types where clarity is needed

## Error Handling

- Use `thiserror` for library error types
- Use `anyhow` for application error handling
- Implement `std::error::Error` for custom error types
- Use `#[from]` to auto-convert errors

## Async Patterns

- Prefer `tokio` for async runtime
- Use `async-trait` for trait methods
- Be careful with `spawn` - ensure proper task cancellation
- Use channels (`tokio::sync::mpsc`) for communication between tasks

## Common Patterns

### Struct Initialization
```rust
// Use struct update syntax
let config = Config {
    timeout: Duration::from_secs(30),
    ..Default::default()
};
```

### Builder Pattern
```rust
impl Client {
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }
}
```

### Type State Pattern
Use type state for compile-time state validation:
```rust
struct Uninitialized;
struct Ready;

struct Client<State = Uninitialized> {
    state: PhantomData<State>,
}
```

## Testing

- Use `tokio::test` for async tests
- Use `tempfile` crate for temporary files in tests
- Use `assert_matches` for pattern matching in assertions
- Mock external dependencies with traits

## Performance

- Use `&str` over `String` for function parameters where possible
- Use `Cow<str>` for flexibility with owned/borrowed strings
- Use `Arc<str>` for shared immutable strings
- Avoid unnecessary cloning - use references

## Dependencies

- Keep dependencies minimal
- Use workspace dependencies for multi-crate projects
- Pin critical dependencies to specific versions
- Regularly run `cargo audit` for security
