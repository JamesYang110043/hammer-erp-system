import { describe, expect, it } from 'vitest';

import { MAX_KEY_LEN } from '../bindings';
import { KEY_MESSAGES, validateKey } from './keyRules';

// 與 crates/shared/src/schema/key.rs 的測試案例相同
describe('validateKey', () => {
  it('accepts_valid_keys', () => {
    for (const key of [
      'a',
      'customer',
      'repair_order',
      'field_2',
      'a_',
      'x1_y2_z3',
      'abcdefghijklmnopqrstuvwxyz0123', // 剛好 30 個字元
    ]) {
      expect(validateKey(key), key).toBeUndefined();
    }
  });

  it('rejects_empty', () => {
    expect(validateKey('')).toBe(KEY_MESSAGES.empty);
  });

  it('rejects_invalid_start', () => {
    for (const key of ['1abc', '_abc', 'Abc', ' abc', '客戶', '-a']) {
      expect(validateKey(key), key).toBe(KEY_MESSAGES.invalidStart);
    }
  });

  it('rejects_invalid_chars', () => {
    for (const key of [
      'repairOrder',
      'repair-order',
      'repair order',
      'abc ',
      'a.b',
      'a客戶',
      'café',
      'a\n',
    ]) {
      expect(validateKey(key), JSON.stringify(key)).toBe(KEY_MESSAGES.invalidChar);
    }
  });

  it('rejects_too_long', () => {
    expect(MAX_KEY_LEN).toBe(30);
    expect(validateKey('a'.repeat(MAX_KEY_LEN + 1))).toBe(KEY_MESSAGES.tooLong);
    expect(KEY_MESSAGES.tooLong).toContain(String(MAX_KEY_LEN));
  });

  it('checks_chars_before_length', () => {
    // 與 Rust 端相同：字元不合法優先於過長
    expect(validateKey(`A${'a'.repeat(40)}`)).toBe(KEY_MESSAGES.invalidStart);
    expect(validateKey(`a-${'a'.repeat(40)}`)).toBe(KEY_MESSAGES.invalidChar);
  });

  it('messages_match_rust', () => {
    // hammer_shared::schema::InvalidKey::message
    expect(KEY_MESSAGES).toEqual({
      empty: '代碼不可為空白',
      tooLong: '代碼不可超過 30 個字元',
      invalidStart: '代碼必須以小寫英文字母開頭',
      invalidChar: '代碼只能使用小寫英文字母、數字與底線',
    });
  });
});
