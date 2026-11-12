import { describe, expect, it } from 'vitest';

import { CommandError, errorMessage, unwrap } from './client';

describe('unwrap', () => {
  it('returns_data_on_ok', async () => {
    await expect(unwrap(Promise.resolve({ status: 'ok' as const, data: 42 }))).resolves.toBe(42);
  });

  it('throws_command_error_on_error', async () => {
    const result = unwrap(
      Promise.resolve({
        status: 'error' as const,
        error: { kind: 'network' as const, message: '無法連線到伺服器' },
      }),
    );
    await expect(result).rejects.toBeInstanceOf(CommandError);
    await expect(result).rejects.toThrow('無法連線到伺服器');
  });
});

describe('errorMessage', () => {
  it('client_outdated_shows_update_hint', () => {
    const err = new CommandError({ kind: 'api', code: 'CLIENT_OUTDATED', message: 'outdated' });
    expect(errorMessage(err)).toBe('程式版本過舊，請更新後再使用');
  });

  it('other_command_error_uses_its_message', () => {
    const err = new CommandError({ kind: 'storage', message: '設定檔寫入失敗' });
    expect(errorMessage(err)).toBe('設定檔寫入失敗');
  });

  it('plain_error_and_unknown_values', () => {
    expect(errorMessage(new Error('boom'))).toBe('boom');
    expect(errorMessage('oops')).toBe('oops');
  });
});
