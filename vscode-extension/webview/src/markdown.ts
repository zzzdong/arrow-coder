import MarkdownIt from 'markdown-it';
import hljs from 'highlight.js/lib/common';

// Matches a fenced-code close/terminator line: 3+ backticks (optionally
// followed by trailing text such as a stray language name like ` ```bash `).
// Models sometimes emit a language name after the closing fence, which fools
// markdown-it into treating it as an unclosed block and swallowing everything
// that follows. We detect such a terminator inside the captured code and cut
// the block off there.
const TERMINATOR_RE = /(^|\n)[ \t]*(`{3,}|~{3,})[ \t]*\S.*$/;

// A single shared markdown-it instance. We deliberately keep `html: false` so
// raw HTML in the model output is NOT rendered — this prevents injected markup
// (and potential XSS) from executing inside the webview. Code fences, lists,
// tables, emphasis etc. still render correctly.
//
// Code blocks are syntax-highlighted with highlight.js and wrapped in a
// structured container (header + copy button + body) so the Vue layer can style
// and wire up the copy action.
const md = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: true,
  highlight: (code: string, lang: string) => {
    // Defensively truncate at the first in-body fence terminator so a stray
    // ` ```bash ` line (or similar) after the real code does not consume the
    // rest of the message as code.
    let body = code;
    const m = TERMINATOR_RE.exec(code);
    if (m) {
      body = code.slice(0, m.index + (m[1] ? m[1].length : 0));
    }

    const language = (lang || 'text').trim();
    const validLang = hljs.getLanguage(language) ? language : null;

    let highlighted: string;
    if (validLang) {
      highlighted = hljs.highlight(body, { language: validLang, ignoreIllegals: true }).value;
    } else {
      highlighted = md.utils.escapeHtml(body);
    }

    const label = md.utils.escapeHtml(language);
    const encoded = md.utils.escapeHtml(body);
    return (
      `<div class="code-block" data-lang="${label}">` +
      `<div class="code-head"><span class="code-lang">${label}</span>` +
      `<button class="code-copy" type="button" data-code="${encoded}">Copy</button></div>` +
      `<pre class="code-body"><code class="hljs language-${validLang || 'text'}">${highlighted}</code></pre>` +
      `</div>`
    );
  },
});

/** Render assistant markdown text to safe HTML. */
export function renderMarkdown(src: string): string {
  return md.render(src ?? '');
}

/**
 * Ensure every opened fenced code block in `src` is properly closed. Models
 * sometimes truncate mid-stream and omit the closing fence; without this the
 * final block swallows all subsequent (non-code) text when re-rendered. Only
 * appends a fence when an odd number of opening fences is present, so already
 * well-formed input is left untouched.
 */
export function ensureClosedFences(src: string): string {
  if (!src) return src;
  const fenceRe = /(^|\n)[ \t]*(`{3,}|~{3,})[ \t]*[^\n]*/g;
  let opens = 0;
  let lastFence = '';
  let m: RegExpExecArray | null;
  while ((m = fenceRe.exec(src)) !== null) {
    opens++;
    lastFence = m[2];
  }
  if (opens % 2 === 1) {
    const needsNewline = !src.endsWith('\n');
    return src + (needsNewline ? '\n' : '') + lastFence + '\n';
  }
  return src;
}
