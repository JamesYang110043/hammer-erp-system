import type { FormRule } from 'antd';

import { MIN_PASSWORD_CHARS } from '../bindings';

export const PASSWORD_TOO_SHORT = `密碼至少需要 ${MIN_PASSWORD_CHARS} 個字元`;
export const PASSWORD_MISMATCH = '兩次輸入的密碼不一致';

/** 密碼規則（docs/architecture.md §6.2）：至少 N 個字元，以字元計算（不是 UTF-16 長度）。 */
export const passwordRule: FormRule = {
  validator: (_, value: string | undefined) =>
    !value || [...value].length >= MIN_PASSWORD_CHARS
      ? Promise.resolve()
      : Promise.reject(new Error(PASSWORD_TOO_SHORT)),
};

/** 「確認密碼」必須與 `field` 欄位相同；Form.Item 要同時設定 `dependencies={[field]}`。 */
export function sameAsRule(field: string): FormRule {
  return ({ getFieldValue }) => ({
    validator: (_, value: string | undefined) =>
      !value || getFieldValue(field) === value
        ? Promise.resolve()
        : Promise.reject(new Error(PASSWORD_MISMATCH)),
  });
}
