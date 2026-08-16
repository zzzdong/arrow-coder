import { describe, it, expect } from 'vitest';
import { fmtTokens, fmtDuration } from './format';

describe('fmtTokens', () => {
  it('returns "0" for invalid / negative input', () => {
    expect(fmtTokens(-5)).toBe('0');
    expect(fmtTokens(NaN)).toBe('0');
    expect(fmtTokens(Infinity)).toBe('0');
  });

  it('passes through small counts unchanged', () => {
    expect(fmtTokens(0)).toBe('0');
    expect(fmtTokens(42)).toBe('42');
    expect(fmtTokens(999)).toBe('999');
  });

  it('abbreviates thousands with one decimal below 10k', () => {
    expect(fmtTokens(1000)).toBe('1.0k');
    expect(fmtTokens(1234)).toBe('1.2k');
  });

  it('abbreviates thousands with no decimal at/above 10k', () => {
    expect(fmtTokens(10000)).toBe('10k');
    expect(fmtTokens(12345)).toBe('12k');
    expect(fmtTokens(999999)).toBe('1000k');
  });

  it('abbreviates millions', () => {
    expect(fmtTokens(1_000_000)).toBe('1.0M');
    expect(fmtTokens(12_345_678)).toBe('12M');
  });
});

describe('fmtDuration', () => {
  it('returns "0ms" for invalid / negative input', () => {
    expect(fmtDuration(-1)).toBe('0ms');
    expect(fmtDuration(NaN)).toBe('0ms');
  });

  it('formats sub-second durations as ms', () => {
    expect(fmtDuration(0)).toBe('0ms');
    expect(fmtDuration(450)).toBe('450ms');
    expect(fmtDuration(999.4)).toBe('999ms');
  });

  it('formats seconds with one decimal', () => {
    expect(fmtDuration(1000)).toBe('1.0s');
    expect(fmtDuration(3500)).toBe('3.5s');
    expect(fmtDuration(59999)).toBe('60.0s');
  });

  it('formats minutes and seconds', () => {
    expect(fmtDuration(60000)).toBe('1m 00s');
    expect(fmtDuration(90000)).toBe('1m 30s');
    expect(fmtDuration(125000)).toBe('2m 05s');
  });
});
