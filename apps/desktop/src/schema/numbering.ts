import dayjs from 'dayjs';
import type { Dayjs } from 'dayjs';

import { TAIPEI_OFFSET_MINUTES } from '../utils/time';

/**
 * 單號規則（docs/architecture.md §8.4 `number_sequences`），與 `hammer_core::numbering` 相同：
 *
 * - `{}` 內為代碼：`yyyy`、`yy`、`MM`、`dd`（可寫在同一個 `{}` 內，例如 `{yyyyMM}`），
 *   或流水號 `{000…}`（3 到 8 個 `0`）
 * - 必須恰好一個流水號；`{}` 以外的文字原樣保留，不可有單獨的 `{`、`}` 或控制字元
 * - 長度上限 {@link MAX_PATTERN_LEN} 個字元
 *
 * 檢查順序與訊息也與 Rust 端相同（`PatternError::message`）。這裡只是即時提示與範例，伺服器仍會檢查。
 */
export const MAX_PATTERN_LEN = 50;
export const MIN_SEQUENCE_DIGITS = 3;
export const MAX_SEQUENCE_DIGITS = 8;

export const PATTERN_MESSAGES = {
  tooLong: `單號規則不可超過 ${MAX_PATTERN_LEN} 個字元`,
  unclosedBrace: '單號規則的「{」沒有對應的「}」',
  unmatchedCloseBrace: '單號規則的「}」沒有對應的「{」',
  controlChar: '單號規則不可包含換行等控制字元',
  unknownCode: (code: string) =>
    `不認得的代碼「{${code}}」；可用的代碼為 {yyyy}、{yy}、{MM}、{dd} 與流水號 {000}`,
  sequenceDigits: `流水號必須是 ${MIN_SEQUENCE_DIGITS} 到 ${MAX_SEQUENCE_DIGITS} 個 0，例如 {0000}`,
  missingSequence: '單號規則必須有一個流水號，例如 {0000}',
  multipleSequences: '單號規則只能有一個流水號',
} as const;

type DateCode = 'yyyy' | 'yy' | 'MM' | 'dd';

/** 較長的放前面（`yyyy` 優先於 `yy`）。 */
const DATE_CODES: readonly DateCode[] = ['yyyy', 'yy', 'MM', 'dd'];

type Part =
  | { kind: 'literal'; text: string }
  | { kind: 'date'; code: DateCode }
  | { kind: 'sequence'; digits: number };

export type ParseResult = { ok: true; parts: Part[] } | { ok: false; message: string };

/** 解析單號規則。不會去除前後空白。 */
export function parseNumberingPattern(pattern: string): ParseResult {
  const chars = Array.from(pattern);
  if (chars.length > MAX_PATTERN_LEN) {
    return { ok: false, message: PATTERN_MESSAGES.tooLong };
  }
  if (/\p{Cc}/u.test(pattern)) {
    return { ok: false, message: PATTERN_MESSAGES.controlChar };
  }

  const parts: Part[] = [];
  let literal = '';
  let rest = pattern;
  while (rest.length > 0) {
    const c = rest[0];
    if (c === '{') {
      const end = rest.indexOf('}');
      if (end < 0) {
        return { ok: false, message: PATTERN_MESSAGES.unclosedBrace };
      }
      const code = rest.slice(1, end);
      if (code.includes('{')) {
        return { ok: false, message: PATTERN_MESSAGES.unclosedBrace };
      }
      if (literal) {
        parts.push({ kind: 'literal', text: literal });
        literal = '';
      }
      const error = parseCode(code, parts);
      if (error) {
        return { ok: false, message: error };
      }
      rest = rest.slice(end + 1);
    } else if (c === '}') {
      return { ok: false, message: PATTERN_MESSAGES.unmatchedCloseBrace };
    } else {
      literal += c;
      rest = rest.slice(1);
    }
  }
  if (literal) {
    parts.push({ kind: 'literal', text: literal });
  }

  const sequences = parts.filter((p) => p.kind === 'sequence').length;
  if (sequences === 0) {
    return { ok: false, message: PATTERN_MESSAGES.missingSequence };
  }
  if (sequences > 1) {
    return { ok: false, message: PATTERN_MESSAGES.multipleSequences };
  }
  return { ok: true, parts };
}

/** `{}` 內的代碼：全是 `0` 為流水號，否則必須由日期代碼組成。回傳錯誤訊息。 */
function parseCode(code: string, parts: Part[]): string | undefined {
  if (/^0+$/.test(code)) {
    if (code.length < MIN_SEQUENCE_DIGITS || code.length > MAX_SEQUENCE_DIGITS) {
      return PATTERN_MESSAGES.sequenceDigits;
    }
    parts.push({ kind: 'sequence', digits: code.length });
    return undefined;
  }
  if (!code) {
    return PATTERN_MESSAGES.unknownCode(code);
  }
  let rest = code;
  while (rest) {
    const token = DATE_CODES.find((t) => rest.startsWith(t));
    if (!token) {
      return PATTERN_MESSAGES.unknownCode(code);
    }
    parts.push({ kind: 'date', code: token });
    rest = rest.slice(token.length);
  }
  return undefined;
}

function renderDate(code: DateCode, date: Dayjs): string {
  switch (code) {
    case 'yyyy':
      return date.format('YYYY');
    case 'yy':
      return date.format('YY');
    case 'MM':
      return date.format('MM');
    case 'dd':
      return date.format('DD');
  }
}

/**
 * 單號規則的範例：
 *
 * - 空白 → `undefined`（不使用單號）
 * - 合法 → `{ ok: true, example }`，以 `now` 的台北日期（D13）產生第 `sequence` 號
 * - 不合法 → `{ ok: false, message }`
 *
 * 與伺服器相同，前後空白會先去除。
 */
export function numberingExample(
  pattern: string | null | undefined,
  now: Dayjs = dayjs(),
  sequence = 1,
): { ok: true; example: string } | { ok: false; message: string } | undefined {
  const trimmed = pattern?.trim();
  if (!trimmed) {
    return undefined;
  }
  const parsed = parseNumberingPattern(trimmed);
  if (!parsed.ok) {
    return parsed;
  }
  const date = now.utcOffset(TAIPEI_OFFSET_MINUTES);
  const example = parsed.parts
    .map((p) => {
      switch (p.kind) {
        case 'literal':
          return p.text;
        case 'date':
          return renderDate(p.code, date);
        case 'sequence':
          return String(sequence).padStart(p.digits, '0');
      }
    })
    .join('');
  return { ok: true, example };
}
