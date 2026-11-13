import { MIN_PASSWORD_CHARS } from '../bindings';

/** 去掉容易看錯的字元（0/O、1/l/I），方便管理者口頭或手寫交給使用者。 */
const ALPHABET = 'ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnpqrstuvwxyz23456789';

/** 臨時密碼長度：比最低要求多 2 個字元。 */
export const TEMP_PASSWORD_LENGTH = MIN_PASSWORD_CHARS + 2;

/** 產生隨機臨時密碼（新增帳號、重設密碼使用）。 */
export function generateTempPassword(length = TEMP_PASSWORD_LENGTH): string {
  const bytes = new Uint32Array(length);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (n) => ALPHABET[n % ALPHABET.length]).join('');
}
