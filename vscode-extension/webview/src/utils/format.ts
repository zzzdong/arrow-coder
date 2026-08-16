// Shared formatting helpers for token counts and durations. Used by
// MessageItem / UsageBar / ContextMeter so the numbers render identically.

/** Format a token count with k/M abbreviation: 1234 → "1.2k", 12345 → "12k". */
export function fmtTokens(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '0';
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 10000 ? 0 : 1)}k`;
  return String(n);
}

/** Format a duration in ms: 450 → "450ms", 3500 → "3.5s", 90000 → "1m 30s". */
export function fmtDuration(ms: number): string {
  if (!Number.isFinite(ms) || ms < 0) return '0ms';
  if (ms < 1000) return `${Math.round(ms)}ms`;
  const s = ms / 1000;
  if (s < 60) return `${s.toFixed(1)}s`;
  const m = Math.floor(s / 60);
  const sec = Math.round(s % 60);
  return `${m}m ${String(sec).padStart(2, '0')}s`;
}
