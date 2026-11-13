import type { FormRule } from 'antd';

import { MAX_KEY_LEN } from '../bindings';

/**
 * 資料類別 `key` 與欄位 `field_key` 的命名規則，與 `hammer_shared::schema::validate_key` 相同：
 *
 * - 只能使用小寫英文字母、數字與底線（`a-z`、`0-9`、`_`）
 * - 開頭必須是英文字母
 * - 長度 1 到 {@link MAX_KEY_LEN} 個字元
 *
 * 檢查順序與訊息也與 Rust 端相同（`InvalidKey::message`）。這裡只是即時提示，伺服器仍會檢查。
 */
export const KEY_MESSAGES = {
  empty: '代碼不可為空白',
  tooLong: `代碼不可超過 ${MAX_KEY_LEN} 個字元`,
  invalidStart: '代碼必須以小寫英文字母開頭',
  invalidChar: '代碼只能使用小寫英文字母、數字與底線',
} as const;

/** 不符合命名規則時回傳訊息；符合時回傳 `undefined`。不會去除前後空白。 */
export function validateKey(key: string): string | undefined {
  if (key.length === 0) {
    return KEY_MESSAGES.empty;
  }
  if (!/^[a-z]/.test(key)) {
    return KEY_MESSAGES.invalidStart;
  }
  if (!/^[a-z0-9_]*$/.test(key)) {
    return KEY_MESSAGES.invalidChar;
  }
  if (key.length > MAX_KEY_LEN) {
    return KEY_MESSAGES.tooLong;
  }
  return undefined;
}

/** antd Form 的代碼規則（輸入時即時檢查）。 */
export const keyRule: FormRule = {
  validator: (_, value: string | undefined) => {
    const message = validateKey(value ?? '');
    return message ? Promise.reject(new Error(message)) : Promise.resolve();
  },
};
