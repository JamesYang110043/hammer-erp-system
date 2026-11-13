import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { ADMIN, startupCommands } from '../test/fixtures';
import { commandError, mockCommands } from '../test/ipc';
import type { CommandHandler } from '../test/ipc';
import { renderWithProviders } from '../test/render';
import { App } from '../App';

/** 以「登入後必須改密碼」的狀態啟動。 */
function renderForced(extra: Record<string, CommandHandler> = {}) {
  window.location.hash = '#/';
  const invoke = mockCommands({
    ...startupCommands({ ...ADMIN, must_change_password: true }),
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

async function fill(current: string, next: string, confirm: string) {
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText('目前的密碼'), current);
  await user.type(screen.getByLabelText('新密碼'), next);
  await user.type(screen.getByLabelText('確認新密碼'), confirm);
  await user.click(screen.getByRole('button', { name: '修改密碼' }));
}

describe('強制改密碼頁', () => {
  it('mismatched_confirmation_is_rejected', async () => {
    const invoke = renderForced();

    await fill('old-password', 'new-password-1', 'new-password-2');

    expect(await screen.findByText('兩次輸入的密碼不一致')).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith('change_password', expect.anything());
  });

  it('too_short_password_is_rejected', async () => {
    const invoke = renderForced();

    await fill('old-password', 'short', 'short');

    expect(await screen.findByText('密碼至少需要 10 個字元')).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith('change_password', expect.anything());
  });

  it('password_length_counts_characters_not_utf16_units', async () => {
    // 9 個字元，但其中的 emoji 佔 2 個 UTF-16 單位，`.length` 為 10
    const invoke = renderForced();

    await fill('old-password', '密碼密碼密碼密碼😀', '密碼密碼密碼密碼😀');

    expect(await screen.findByText('密碼至少需要 10 個字元')).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith('change_password', expect.anything());
  });

  it('server_validation_error_is_shown', async () => {
    renderForced({
      change_password: () =>
        commandError({ kind: 'api', code: 'VALIDATION', message: '目前的密碼不正確' }),
    });

    await fill('wrong-password', 'new-password-1', 'new-password-1');

    expect(await screen.findByText('目前的密碼不正確')).toBeInTheDocument();
  });

  it('success_enters_main_screen', async () => {
    const invoke = renderForced({ change_password: () => null });

    await fill('old-password', 'new-password-1', 'new-password-1');

    expect(await screen.findByRole('menu')).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('change_password', {
      req: { current_password: 'old-password', new_password: 'new-password-1' },
    });
  });
});
