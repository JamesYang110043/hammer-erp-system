import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it } from 'vitest';

import { App } from './App';
import type { CurrentUser } from './bindings';
import { ADMIN, VIEWER, startupCommands } from './test/fixtures';
import { commandError, mockCommands } from './test/ipc';
import type { CommandHandler } from './test/ipc';
import { renderWithProviders } from './test/render';

// antd 會在兩個中文字的按鈕中間插入空白（「登 入」）
const LOGIN_BUTTON = /^登\s?入$/;

// 路由是模組層級的 hash router，每個測試從首頁開始
beforeEach(() => {
  window.location.hash = '#/';
});

function renderApp(user: CurrentUser | null, extra: Record<string, CommandHandler> = {}) {
  const invoke = mockCommands({ ...startupCommands(user), ...extra });
  const result = renderWithProviders(<App />);
  return { invoke, ...result };
}

async function submitLogin(username = 'admin', password = 'correct-password') {
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText('帳號'), username);
  await user.type(screen.getByLabelText('密碼'), password);
  await user.click(screen.getByRole('button', { name: LOGIN_BUTTON }));
}

/** 主畫面右上角的使用者選單。 */
async function openUserMenu(displayName: string) {
  await userEvent.click(await screen.findByRole('button', { name: new RegExp(displayName) }));
}

describe('啟動流程', () => {
  it('needs_setup_shows_setup_page_and_logs_in_after_create', async () => {
    const invoke = mockCommands({
      ...startupCommands(null, { needsSetup: true }),
      setup_admin: () => ADMIN,
    });
    const user = userEvent.setup();
    renderWithProviders(<App />);

    await user.type(await screen.findByLabelText('帳號'), ' admin ');
    await user.type(screen.getByLabelText('顯示名稱'), '王管理');
    await user.type(screen.getByLabelText('密碼'), 'first-password');
    await user.type(screen.getByLabelText('確認密碼'), 'first-password');
    await user.click(screen.getByRole('button', { name: '建立並登入' }));

    expect(await screen.findByRole('button', { name: /王管理/ })).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('setup_admin', {
      req: { username: 'admin', display_name: '王管理', password: 'first-password' },
    });
    // 已有帳號時不會呼叫 current_user
    expect(invoke).not.toHaveBeenCalledWith('current_user', expect.anything());
  });

  it('valid_token_goes_straight_to_main_screen', async () => {
    renderApp(ADMIN);
    expect(await screen.findByRole('button', { name: /王管理/ })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: LOGIN_BUTTON })).not.toBeInTheDocument();
  });

  it('no_token_shows_login_page', async () => {
    renderApp(null);
    expect(await screen.findByRole('button', { name: LOGIN_BUTTON })).toBeInTheDocument();
  });

  it('expired_token_on_startup_shows_login_with_notice', async () => {
    renderApp(null, {
      current_user: () =>
        commandError({ kind: 'sessionExpired', message: '登入已逾時，請重新登入' }),
    });
    expect(await screen.findByText('登入已逾時，請重新登入')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: LOGIN_BUTTON })).toBeInTheDocument();
  });

  it('server_unreachable_shows_error_with_retry', async () => {
    renderApp(null, {
      setup_status: () =>
        commandError({
          kind: 'network',
          message: '無法連線到伺服器，請確認伺服器已啟動、網址正確',
        }),
    });
    expect(
      await screen.findByText('無法連線到伺服器，請確認伺服器已啟動、網址正確'),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /^重\s?試$/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /連線設定/ })).toBeInTheDocument();
  });
});

describe('登入頁', () => {
  it('login_success_enters_main_screen', async () => {
    const { invoke } = renderApp(null, { login: () => ADMIN });

    await submitLogin(' admin ', 'correct-password');

    expect(await screen.findByRole('button', { name: /王管理/ })).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('login', {
      req: { username: 'admin', password: 'correct-password' },
    });
  });

  it('wrong_password_shows_error', async () => {
    renderApp(null, {
      login: () => commandError({ kind: 'api', code: 'UNAUTHORIZED', message: '帳號或密碼錯誤' }),
    });

    await submitLogin('admin', 'wrong-password');

    expect(await screen.findByText('帳號或密碼錯誤')).toBeInTheDocument();
    // 登入失敗不是「登入逾時」，留在登入頁
    expect(screen.queryByText('登入已逾時，請重新登入')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: LOGIN_BUTTON })).toBeInTheDocument();
  });

  it('locked_account_shows_unlock_time', async () => {
    renderApp(null, {
      login: () =>
        commandError({
          kind: 'api',
          code: 'ACCOUNT_LOCKED',
          message: '登入失敗次數過多，帳號已暫時鎖定，請於 2026-11-13 09:05 後再試',
          details: { locked_until: '2026-11-13T01:05:00Z' },
        }),
    });

    await submitLogin();

    expect(
      await screen.findByText('登入失敗次數過多，帳號已暫時鎖定，請於 2026-11-13 09:05 後再試'),
    ).toBeInTheDocument();
  });

  it('disabled_account_shows_message', async () => {
    renderApp(null, {
      login: () =>
        commandError({
          kind: 'api',
          code: 'ACCOUNT_DISABLED',
          message: '此帳號已停用，請聯絡管理者',
        }),
    });

    await submitLogin();

    expect(await screen.findByText('此帳號已停用，請聯絡管理者')).toBeInTheDocument();
  });

  it('must_change_password_after_login_shows_change_password_page', async () => {
    renderApp(null, { login: () => ({ ...ADMIN, must_change_password: true }) });

    await submitLogin();

    expect(await screen.findByLabelText('確認新密碼')).toBeInTheDocument();
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
  });
});

describe('登入逾時', () => {
  it('session_expired_on_any_command_returns_to_login', async () => {
    renderApp(ADMIN, {
      change_password: () =>
        commandError({ kind: 'sessionExpired', message: '登入已逾時，請重新登入' }),
    });
    const user = userEvent.setup();

    await openUserMenu('王管理');
    await user.click(await screen.findByRole('menuitem', { name: /修改密碼/ }));
    const dialog = await screen.findByRole('dialog');
    await user.type(within(dialog).getByLabelText('目前的密碼'), 'old-password');
    await user.type(within(dialog).getByLabelText('新密碼'), 'new-password-1');
    await user.type(within(dialog).getByLabelText('確認新密碼'), 'new-password-1');
    await user.click(within(dialog).getByRole('button', { name: '修改密碼' }));

    expect(await screen.findByText('登入已逾時，請重新登入')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: LOGIN_BUTTON })).toBeInTheDocument();
  });

  it('password_change_required_from_server_shows_change_password_page', async () => {
    renderApp(ADMIN, {
      logout: () =>
        commandError({
          kind: 'api',
          code: 'PASSWORD_CHANGE_REQUIRED',
          message: '請先修改密碼',
        }),
    });

    await openUserMenu('王管理');
    await userEvent.click(await screen.findByRole('menuitem', { name: /登出/ }));

    expect(await screen.findByLabelText('確認新密碼')).toBeInTheDocument();
  });
});

describe('主畫面', () => {
  it('viewer_without_user_manage_cannot_see_user_management_menu', async () => {
    renderApp(VIEWER);

    const menu = await screen.findByRole('menu');
    expect(within(menu).getByRole('menuitem', { name: /進出貨/ })).toBeInTheDocument();
    expect(within(menu).queryByRole('menuitem', { name: /帳號管理/ })).not.toBeInTheDocument();
    expect(within(menu).queryByRole('menuitem', { name: /系統設定/ })).not.toBeInTheDocument();
  });

  it('admin_sees_user_management_menu', async () => {
    renderApp(ADMIN);

    const menu = await screen.findByRole('menu');
    expect(within(menu).getByRole('menuitem', { name: /帳號管理/ })).toBeInTheDocument();
    expect(within(menu).getByRole('menuitem', { name: /系統設定/ })).toBeInTheDocument();
  });

  it('viewer_opening_user_management_route_sees_forbidden', async () => {
    window.location.hash = '#/users';
    renderApp(VIEWER);

    expect(await screen.findByText('您沒有使用此功能的權限')).toBeInTheDocument();
  });

  it('logout_returns_to_login_page', async () => {
    const { invoke } = renderApp(ADMIN, { logout: () => null });

    await openUserMenu('王管理');
    await userEvent.click(await screen.findByRole('menuitem', { name: /登出/ }));

    expect(await screen.findByRole('button', { name: LOGIN_BUTTON })).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('logout', {});
  });
});
