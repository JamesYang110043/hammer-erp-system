import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import { CONFLICT_MESSAGE } from '../../api/errors';
import { ADMIN, ROLES, managedUser, startupCommands } from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';

const EDITOR = managedUser();
const ADMIN_ROW = managedUser({
  id: ADMIN.id,
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  role_name: '管理者',
  version: 1,
});
const LAST_ADMIN_MESSAGE =
  '系統至少需要保留一個有效的管理者，無法停用、刪除最後一個管理者或變更其角色';

/** 以管理者登入，開啟帳號管理頁。 */
function renderUsersPage(extra: Record<string, CommandHandler> = {}) {
  window.location.hash = '#/users';
  const invoke = mockCommands({
    ...startupCommands(ADMIN),
    list_users: () => [ADMIN_ROW, EDITOR],
    list_roles: () => ROLES,
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

async function rowOf(username: string) {
  const cell = await screen.findByRole('cell', { name: username });
  return within(cell.closest('tr')!);
}

async function openCreateDialog() {
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: /新增帳號/ }));
  const dialog = within(await screen.findByRole('dialog'));
  return { user, dialog };
}

async function chooseRole(user: ReturnType<typeof userEvent.setup>, label: string) {
  await user.click(screen.getByLabelText('角色'));
  await user.click(await screen.findByTitle(label));
}

describe('帳號管理頁', () => {
  it('lists_users_with_status_and_last_login', async () => {
    renderUsersPage({
      list_users: () => [
        EDITOR,
        managedUser({ id: 'u2', username: 'locked1', is_locked: true, last_login_at: null }),
        managedUser({ id: 'u3', username: 'disabled1', is_active: false }),
      ],
    });

    const editor = await rowOf('editor1');
    expect(editor.getByText('陳編輯')).toBeInTheDocument();
    expect(editor.getByText('可編輯者')).toBeInTheDocument();
    expect(editor.getByText('正常')).toBeInTheDocument();
    // 以台北時間顯示
    expect(editor.getByText('2026-11-13 09:05:30')).toBeInTheDocument();

    const locked = await rowOf('locked1');
    expect(locked.getByText(/^鎖定/)).toBeInTheDocument();
    expect(locked.getByText('從未登入')).toBeInTheDocument();
    expect(locked.getByRole('button', { name: '解除鎖定' })).toBeInTheDocument();
    expect(editor.queryByRole('button', { name: '解除鎖定' })).not.toBeInTheDocument();

    expect((await rowOf('disabled1')).getByText('停用')).toBeInTheDocument();
  });

  describe('新增帳號表單', () => {
    it('required_fields_are_validated', async () => {
      const invoke = renderUsersPage();
      const { user, dialog } = await openCreateDialog();

      await user.clear(dialog.getByLabelText('臨時密碼'));
      await user.click(dialog.getByRole('button', { name: /建\s*立/ }));

      expect(await dialog.findByText('請輸入帳號')).toBeInTheDocument();
      expect(dialog.getByText('請輸入顯示名稱')).toBeInTheDocument();
      expect(dialog.getByText('請選擇角色')).toBeInTheDocument();
      expect(dialog.getByText('請輸入臨時密碼')).toBeInTheDocument();
      expect(invoke).not.toHaveBeenCalledWith('create_user', expect.anything());
    });

    it('whitespace_only_username_is_rejected', async () => {
      const invoke = renderUsersPage();
      const { user, dialog } = await openCreateDialog();

      await user.type(dialog.getByLabelText('帳號'), '   ');
      await user.type(dialog.getByLabelText('顯示名稱'), '林可編');
      await chooseRole(user, '可編輯者');
      await user.click(dialog.getByRole('button', { name: /建\s*立/ }));

      expect(await dialog.findByText('請輸入帳號')).toBeInTheDocument();
      expect(invoke).not.toHaveBeenCalledWith('create_user', expect.anything());
    });

    it('too_short_password_is_rejected', async () => {
      const invoke = renderUsersPage();
      const { user, dialog } = await openCreateDialog();

      await user.type(dialog.getByLabelText('帳號'), 'editor2');
      await user.type(dialog.getByLabelText('顯示名稱'), '林可編');
      await chooseRole(user, '可編輯者');
      const password = dialog.getByLabelText('臨時密碼');
      await user.clear(password);
      await user.type(password, 'short');
      await user.click(dialog.getByRole('button', { name: /建\s*立/ }));

      expect(await dialog.findByText('密碼至少需要 10 個字元')).toBeInTheDocument();
      expect(invoke).not.toHaveBeenCalledWith('create_user', expect.anything());
    });

    it('generated_password_meets_rules_and_is_shown_after_create', async () => {
      const invoke = renderUsersPage({
        create_user: (args) =>
          managedUser({ username: (args.req as { username: string }).username }),
      });
      const { user, dialog } = await openCreateDialog();

      const generated = (dialog.getByLabelText('臨時密碼') as HTMLInputElement).value;
      expect([...generated].length).toBeGreaterThanOrEqual(10);

      await user.type(dialog.getByLabelText('帳號'), 'editor2');
      await user.type(dialog.getByLabelText('顯示名稱'), '林可編');
      await chooseRole(user, '可編輯者');
      await user.click(dialog.getByRole('button', { name: /建\s*立/ }));

      expect(await dialog.findByText('帳號已建立')).toBeInTheDocument();
      expect(dialog.getByText(generated)).toBeInTheDocument();
      expect(dialog.getByText(/請將臨時密碼交給使用者/)).toBeInTheDocument();
      expect(invoke).toHaveBeenCalledWith('create_user', {
        req: { username: 'editor2', display_name: '林可編', role: 'editor', password: generated },
      });
    });

    it('validation_error_with_field_is_shown_under_field', async () => {
      renderUsersPage({
        create_user: () =>
          commandError({
            kind: 'api',
            code: 'VALIDATION',
            message: '此帳號已被使用',
            details: { field: 'username' },
          }),
      });
      const { user, dialog } = await openCreateDialog();

      await user.type(dialog.getByLabelText('帳號'), 'editor1');
      await user.type(dialog.getByLabelText('顯示名稱'), '陳編輯');
      await chooseRole(user, '可編輯者');
      await user.click(dialog.getByRole('button', { name: /建\s*立/ }));

      const error = await dialog.findByText('此帳號已被使用');
      // 顯示在「帳號」欄位下方
      expect(error.closest('.ant-form-item')).toContainElement(dialog.getByLabelText('帳號'));
    });
  });

  describe('錯誤訊息', () => {
    it('conflict_on_edit_shows_reload_message', async () => {
      const invoke = renderUsersPage({
        update_user: () =>
          commandError({
            kind: 'api',
            code: 'CONFLICT',
            message: '此資料已被其他人修改，請重新載入',
          }),
      });
      const user = userEvent.setup();

      await user.click((await rowOf('editor1')).getByRole('button', { name: '編輯' }));
      const dialog = within(await screen.findByRole('dialog'));
      const name = dialog.getByLabelText('顯示名稱');
      await user.clear(name);
      await user.type(name, '陳大編');
      await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));

      expect(await dialog.findByText(CONFLICT_MESSAGE)).toBeInTheDocument();
      // 只送出有變更的欄位，並帶上讀取時的版本號
      expect(invoke).toHaveBeenCalledWith('update_user', {
        id: EDITOR.id,
        req: { version: 3, display_name: '陳大編', role: null, is_active: null },
      });
    });

    it('last_admin_error_on_edit_is_shown_as_is', async () => {
      const invoke = renderUsersPage({
        update_user: () =>
          commandError({
            kind: 'api',
            code: 'VALIDATION',
            message: LAST_ADMIN_MESSAGE,
            details: { reason: 'last_admin' },
          }),
      });
      const user = userEvent.setup();

      await user.click((await rowOf('admin')).getByRole('button', { name: '編輯' }));
      const dialog = within(await screen.findByRole('dialog'));
      await user.click(dialog.getByRole('switch'));
      await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));

      expect(await dialog.findByText(LAST_ADMIN_MESSAGE)).toBeInTheDocument();
      expect(invoke).toHaveBeenCalledWith('update_user', {
        id: ADMIN.id,
        req: { version: 1, display_name: null, role: null, is_active: false },
      });
    });

    it('last_admin_error_on_delete_is_shown_as_is', async () => {
      const invoke = renderUsersPage({
        delete_user: () =>
          commandError({
            kind: 'api',
            code: 'VALIDATION',
            message: LAST_ADMIN_MESSAGE,
            details: { reason: 'last_admin' },
          }),
      });
      const user = userEvent.setup();

      await user.click((await rowOf('admin')).getByRole('button', { name: '刪除' }));
      const confirm = await screen.findByRole('tooltip');
      await user.click(within(confirm).getByRole('button', { name: /刪\s*除/ }));

      expect(await screen.findByText(LAST_ADMIN_MESSAGE)).toBeInTheDocument();
      expect(invoke).toHaveBeenCalledWith('delete_user', { id: ADMIN.id, version: 1 });
    });

    it('conflict_on_delete_shows_reload_message', async () => {
      renderUsersPage({
        delete_user: () =>
          commandError({
            kind: 'api',
            code: 'CONFLICT',
            message: '此資料已被其他人修改，請重新載入',
          }),
      });
      const user = userEvent.setup();

      await user.click((await rowOf('editor1')).getByRole('button', { name: '刪除' }));
      const confirm = await screen.findByRole('tooltip');
      await user.click(within(confirm).getByRole('button', { name: /刪\s*除/ }));

      expect(await screen.findByText(CONFLICT_MESSAGE)).toBeInTheDocument();
    });
  });

  it('reset_password_shows_temporary_password', async () => {
    const invoke = renderUsersPage({
      reset_user_password: () => managedUser({ must_change_password: true, version: 4 }),
    });
    const user = userEvent.setup();

    await user.click((await rowOf('editor1')).getByRole('button', { name: '重設密碼' }));
    const dialog = within(await screen.findByRole('dialog'));
    const password = dialog.getByLabelText('臨時密碼');
    await user.clear(password);
    await user.type(password, 'Temp-Pass-2026');
    await user.click(dialog.getByRole('button', { name: /重設密碼/ }));

    expect(await dialog.findByText('密碼已重設')).toBeInTheDocument();
    expect(dialog.getByText('Temp-Pass-2026')).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('reset_user_password', {
      id: EDITOR.id,
      req: { new_password: 'Temp-Pass-2026' },
    });
  });

  it('unlock_calls_command_and_refreshes_list', async () => {
    const locked = managedUser({ is_locked: true });
    const invoke = renderUsersPage({
      list_users: () => [locked],
      unlock_user: () => managedUser(),
    });
    const user = userEvent.setup();

    await user.click((await rowOf('editor1')).getByRole('button', { name: '解除鎖定' }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith('unlock_user', { id: locked.id }));
    await waitFor(() =>
      expect(invoke.mock.calls.filter(([cmd]) => cmd === 'list_users')).toHaveLength(2),
    );
  });
});
