import type { FormInstance } from 'antd';

import { mutationErrorMessage, validationFieldError } from '../../api/errors';

/**
 * 送出表單失敗時顯示錯誤：
 *
 * - `422` 且有 `details.field`，而且表單有該欄位 → 顯示在欄位下方，回傳 `undefined`
 * - 其他（`409`、最後一個管理者保護等）→ 回傳要顯示在表單上方的訊息
 */
export function applyFormError<T>(form: FormInstance<T>, err: unknown): string | undefined {
  const fieldError = validationFieldError(err);
  if (fieldError && form.getFieldInstance(fieldError.field as never) !== undefined) {
    form.setFields([{ name: fieldError.field as never, errors: [fieldError.message] }]);
    return undefined;
  }
  return mutationErrorMessage(err);
}
