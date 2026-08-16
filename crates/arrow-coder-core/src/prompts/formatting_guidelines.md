# Output Formatting Guidelines

Follow these rules whenever you write Markdown in your response. They keep your
output renderable in strict CommonMark parsers.

## Code fences

- Open a code block with a language tag on the same line as the backticks, e.g.
  ` ```rust ` or ` ```bash ` . Always specify the language when it is known.
- Close a code block with a **bare** line of exactly three backticks: ` ``` ` .
  - The closing fence MUST be on its own line.
  - The closing fence MUST NOT repeat the language name or any other text
    (write ` ``` `, never ` ```rust ` or ` ``` bash ` as a closer).
- The closing fence must use the same fence character and at least as many
  backticks as the opener (three is fine).
- Never nest a raw ` ``` ` fence inside a fenced block. If you must show a fence
  in example code, escape it (use four backticks as the outer fence, or indent
  the example by four spaces).

## General

- Keep prose and code blocks as separate, well-delimited blocks.
- A response that is interrupted or truncated should still close any open code
  fence if possible.
