# edit

Precise, surgical file edits. Prefer this over `write_file` for any change to an existing file — it keeps the content you transmit tiny, which avoids corruption.

## How it works
Replace every occurrence of `old_string` with `new_string`. Text is matched **literally** (no regex, no escaping). Indentation and surrounding whitespace matter exactly.

## Parameters
- `path` (string, required): file to edit.
- `old_string` (string, required, non-empty): exact literal text to find. Must match the file content exactly, including whitespace and indentation.
- `new_string` (string, required): the replacement. Use `""` to delete.
- `replace_all` (boolean, optional, default `false`): if `false`, `old_string` must appear **exactly once**; if it appears 0 or >1 times the edit fails with a clear error. Set `true` only when you intentionally want every match replaced.

## CRITICAL rules
1. **Always `read` the file first.** The `old_string` must exactly match the current on-disk content. If you edit from memory, whitespace/field drift will break the match (the tool will error, it will NOT silently corrupt the file).
2. **Include enough surrounding context** in `old_string` to be unique. A single line like `}` or `let x = 0;` matches many places — include the enclosing function signature or a few lines.
3. **Never rewrite an entire file via `edit`.** Send only the changed lines. For a wholly new file use `write_file`; for a large rewrite of an existing file, still prefer one or more small `edit` calls over pasting the whole file.
4. If the edit fails because `old_string` was not unique or not found, re-`read` the file and resend with more context — do not fall back to `write_file` and paste the entire file.

## Examples
Edit one function body:
```json
{
  "path": "src/foo.rs",
  "old_string": "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}",
  "new_string": "fn add(a: i32, b: i32) -> i32 {\n    a.checked_add(b).unwrap_or(0)\n}",
  "replace_all": false
}
```

Delete a line:
```json
{
  "path": "src/foo.rs",
  "old_string": "    eprintln!(\"debug: {:?}\", val);\n",
  "new_string": ""
}
```
