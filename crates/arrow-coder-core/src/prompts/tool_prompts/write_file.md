# write_file

Create a file or completely overwrite an existing one with the exact `content` you provide.

## Parameters
- `path` (string, required): file to write. Parent directories are created automatically.
- `content` (string, required): the **entire** file contents.

## When to use
- Creating a brand-new file (modules, configs, scripts).
- Wholesale regeneration where `edit` would need many disjoint changes.

## When NOT to use (critical)
- **Editing an existing file**: prefer `edit` with a small `old_string`/`new_string`. Emitting an entire file as a single JSON string is fragile — long Rust sources frequently get corrupted in transit (dropped characters, missing fields, broken brace/paren balance), which then writes a broken file to disk.
- If your change touches only part of a file, always `edit` instead.

## Integrity
The file is written atomically (temp file + rename) and verified byte-for-byte after write. If verification fails the original file (if any) is left intact and the tool reports an error rather than leaving a corrupted file.

## Example
```json
{
  "path": "src/widget.rs",
  "content": "pub struct Widget;\n\nimpl Widget {\n    pub fn new() -> Self { Widget }\n}\n"
}
```
