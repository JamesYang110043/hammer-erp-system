import dayjs from 'dayjs';
import { describe, expect, it } from 'vitest';

import { PATTERN_MESSAGES, numberingExample, parseNumberingPattern } from './numbering';

// 台北時間 2026-11-05 09:00
const NOW = dayjs('2026-11-05T01:00:00Z');

function example(pattern: string, sequence?: number): string {
  const result = numberingExample(pattern, NOW, sequence);
  if (!result?.ok) {
    throw new Error(`${pattern} 應該合法：${JSON.stringify(result)}`);
  }
  return result.example;
}

function error(pattern: string): string {
  const result = parseNumberingPattern(pattern);
  if (result.ok) {
    throw new Error(`${pattern} 應該不合法`);
  }
  return result.message;
}

describe('numberingExample', () => {
  it('renders_date_codes_and_sequence', () => {
    expect(example('SO{yyyyMM}-{0000}')).toBe('SO202611-0001');
    expect(example('{yy}{MM}{dd}{000}')).toBe('261105001');
    expect(example('維修-{yyyy}/{MM}/{dd}-{00000000}')).toBe('維修-2026/11/05-00000001');
    expect(example('A{000}')).toBe('A001');
  });

  it('uses_taipei_date', () => {
    // UTC 2026-11-30 17:00 = 台北 2026-12-01 01:00
    const result = numberingExample('{yyyyMMdd}-{000}', dayjs('2026-11-30T17:00:00Z'));
    expect(result).toEqual({ ok: true, example: '20261201-001' });
  });

  it('does_not_truncate_overflowing_sequence', () => {
    expect(example('{000}', 1000)).toBe('1000');
  });

  it('blank_pattern_means_no_numbering', () => {
    expect(numberingExample('', NOW)).toBeUndefined();
    expect(numberingExample('   ', NOW)).toBeUndefined();
    expect(numberingExample(null, NOW)).toBeUndefined();
  });

  it('trims_surrounding_whitespace', () => {
    expect(example('  SO{0000} ')).toBe('SO0001');
  });

  it('returns_message_for_invalid_pattern', () => {
    expect(numberingExample('SO{yyyy}', NOW)).toEqual({
      ok: false,
      message: PATTERN_MESSAGES.missingSequence,
    });
  });
});

describe('parseNumberingPattern', () => {
  it('rejects_invalid_patterns_with_server_messages', () => {
    expect(error('X'.repeat(47) + '{000}')).toBe(PATTERN_MESSAGES.tooLong);
    expect(error('SO{0000')).toBe(PATTERN_MESSAGES.unclosedBrace);
    expect(error('SO{{0000}')).toBe(PATTERN_MESSAGES.unclosedBrace);
    expect(error('SO}{0000}')).toBe(PATTERN_MESSAGES.unmatchedCloseBrace);
    expect(error('SO\n{0000}')).toBe(PATTERN_MESSAGES.controlChar);
    expect(error('{yyyyQ}{000}')).toBe(PATTERN_MESSAGES.unknownCode('yyyyQ'));
    expect(error('{}{000}')).toBe(PATTERN_MESSAGES.unknownCode(''));
    expect(error('{00}')).toBe(PATTERN_MESSAGES.sequenceDigits);
    expect(error('{000000000}')).toBe(PATTERN_MESSAGES.sequenceDigits);
    expect(error('SO{yyyy}')).toBe(PATTERN_MESSAGES.missingSequence);
    expect(error('{000}-{000}')).toBe(PATTERN_MESSAGES.multipleSequences);
  });

  it('counts_characters_not_bytes', () => {
    // 46 個中文字 + `{000}` = 51 字元
    expect(error('單'.repeat(46) + '{000}')).toBe(PATTERN_MESSAGES.tooLong);
    expect(parseNumberingPattern('單'.repeat(45) + '{000}').ok).toBe(true);
  });
});
