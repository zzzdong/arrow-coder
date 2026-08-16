Use `grep` to recursively search for a regular expression pattern in files.

**Key characteristics:**
- Searches file contents using regex patterns
- Supports filtering by file glob patterns
- Returns matching file paths by default
- Can return matching lines with context

**Arguments:**
- `pattern`: The regex pattern to search for
- `path`: Directory or file to search (default: current working directory)
- `glob`: File pattern to filter by (e.g., "*.rs", "**/*.toml")
- `output_mode`: "files_with_matches" (default), "content", or "count"
- `head_limit`: Limit number of results

**Best practices:**
- Use specific patterns to narrow results
- Combine with `glob` to search specific file types
- Use `output_mode: "content"` with `-n` to see line numbers
- Use `head_limit` to avoid overwhelming output

**Examples:**

```python
# Find all Rust files containing "async fn"
grep(pattern="async fn", glob="*.rs")

# Search for function definitions with line numbers
grep(pattern="^fn ", output_mode="content", head_limit=20)

# Count occurrences
grep(pattern="TODO", output_mode="count")
```
