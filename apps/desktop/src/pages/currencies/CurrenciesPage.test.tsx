import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import { CONFLICT_TITLE } from '../../components/ConflictAlert';
import {
  ADMIN,
  CURRENCIES,
  EDITOR,
  VIEWER,
  currencyRow,
  startupCommands,
} from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import type { CurrentUser } from '../../bindings';

/** 頁面第一次載入（啟動流程 → 路由 → 列表）的等待上限（CI 機器較慢）。 */
const PAGE_LOAD_TIMEOUT = 5000;

function renderAt(
  hash: string,
  user: CurrentUser = ADMIN,
  extra: Record<string, CommandHandler> = {},
) {
  window.location.hash = hash;
  const invoke = mockCommands({
    ...startupCommands(user),
    list_currencies: () => CURRENCIES,
    list_exchange_rates: () => ({ items: [], total: 0, page: 1, page_size: 50 }),
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

function calls(invoke: ReturnType<typeof mockCommands>, command: string) {
  return invoke.mock.calls.filter(([cmd]) => cmd === command).map(([, args]) => args);
}

async function rowOf(code: string) {
  const cell = await screen.findByText(code, { selector: 'td' }, { timeout: PAGE_LOAD_TIMEOUT });
  return within(cell.closest('tr')!);
}

function save(dialog: ReturnType<typeof within>) {
  return dialog.getByRole('button', { name: /儲\s*存/ });
}

describe('幣別、匯率選單與權限', () => {
  it('admin_sees_currencies_and_rates_under_master_data', async () => {
    renderAt('#/connection');
    const menu = await screen.findByRole('menu', undefined, { timeout: PAGE_LOAD_TIMEOUT });
    const group = within(menu)
      .getByText('基本資料', { selector: '.ant-menu-item-group-title' })
      .closest('li')!;
    expect(within(group).getByRole('menuitem', { name: /幣別/ })).toBeInTheDocument();
    expect(within(group).getByRole('menuitem', { name: /匯率/ })).toBeInTheDocument();
  });

  it.each([
    ['editor', EDITOR],
    ['viewer', VIEWER],
  ])('%s_sees_rates_but_not_currencies', async (_, user) => {
    renderAt('#/connection', user);
    const menu = await screen.findByRole('menu', undefined, { timeout: PAGE_LOAD_TIMEOUT });
    expect(within(menu).getByRole('menuitem', { name: /匯率/ })).toBeInTheDocument();
    expect(within(menu).queryByRole('menuitem', { name: /幣別/ })).not.toBeInTheDocument();
  });

  it('editor_opening_currencies_route_sees_forbidden', async () => {
    const invoke = renderAt('#/currencies', EDITOR);
    expect(
      await screen.findByText('您沒有使用此功能的權限', undefined, { timeout: PAGE_LOAD_TIMEOUT }),
    ).toBeInTheDocument();
    expect(calls(invoke, 'list_currencies')).toEqual([]);
  });
});

describe('幣別管理頁', () => {
  it('lists_all_currencies_including_inactive', async () => {
    const invoke = renderAt('#/currencies');

    const twd = await rowOf('TWD');
    expect(twd.getByText('本位幣')).toBeInTheDocument();
    // 本位幣不可停用
    expect(twd.queryByRole('button', { name: '停用' })).not.toBeInTheDocument();
    const jpy = await rowOf('JPY');
    expect(jpy.getByText('停用')).toBeInTheDocument();
    expect(jpy.getByRole('button', { name: '啟用' })).toBeInTheDocument();
    expect((await rowOf('USD')).getByRole('button', { name: '停用' })).toBeInTheDocument();
    expect(calls(invoke, 'list_currencies')).toContainEqual({ includeInactive: true });
  });

  it('creates_currency_with_uppercased_code', async () => {
    const invoke = renderAt('#/currencies', ADMIN, {
      create_currency: () => currencyRow({ code: 'GBP', name: '英鎊' }),
    });
    const user = userEvent.setup();
    await rowOf('USD');

    await user.click(screen.getByRole('button', { name: /新增幣別/ }));
    const dialog = within(await screen.findByRole('dialog'));
    await user.type(dialog.getByLabelText('代碼'), 'gbp');
    expect(dialog.getByLabelText('代碼')).toHaveValue('GBP');
    await user.type(dialog.getByLabelText('名稱'), ' 英鎊 ');
    await user.type(dialog.getByLabelText('符號'), '£');
    await user.click(save(dialog));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('create_currency', {
        req: { code: 'GBP', name: '英鎊', symbol: '£', decimal_places: 2 },
      }),
    );
    expect(await screen.findByText('已新增幣別 GBP')).toBeInTheDocument();
  });

  it('invalid_code_is_rejected_before_sending', async () => {
    const invoke = renderAt('#/currencies');
    const user = userEvent.setup();
    await rowOf('USD');

    await user.click(screen.getByRole('button', { name: /新增幣別/ }));
    const dialog = within(await screen.findByRole('dialog'));
    await user.type(dialog.getByLabelText('代碼'), 'G1');
    await user.type(dialog.getByLabelText('名稱'), '英鎊');
    await user.click(save(dialog));

    expect(await dialog.findByText('幣別代碼必須是 3 個英文字母')).toBeInTheDocument();
    expect(calls(invoke, 'create_currency')).toEqual([]);
  });

  it('duplicate_code_error_is_shown_on_field', async () => {
    renderAt('#/currencies', ADMIN, {
      create_currency: () =>
        commandError({
          kind: 'api',
          code: 'VALIDATION',
          message: '幣別代碼 USD 已存在',
          details: { field: 'code' },
        }),
    });
    const user = userEvent.setup();
    await rowOf('USD');

    await user.click(screen.getByRole('button', { name: /新增幣別/ }));
    const dialog = within(await screen.findByRole('dialog'));
    await user.type(dialog.getByLabelText('代碼'), 'usd');
    await user.type(dialog.getByLabelText('名稱'), '美元');
    await user.click(save(dialog));

    const error = await dialog.findByText('幣別代碼 USD 已存在');
    expect(error.closest('.ant-form-item')).toHaveTextContent('代碼');
  });

  it('edit_sends_changed_fields_with_version', async () => {
    const invoke = renderAt('#/currencies', ADMIN, {
      update_currency: () => currencyRow({ symbol: null, version: 3 }),
    });
    const user = userEvent.setup();

    await user.click((await rowOf('USD')).getByRole('button', { name: '編輯' }));
    const dialog = within(await screen.findByRole('dialog'));
    expect(dialog.getByLabelText('代碼')).toBeDisabled();
    await user.clear(dialog.getByLabelText('符號'));
    await user.click(save(dialog));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('update_currency', {
        code: 'USD',
        req: { version: 2, name: null, symbol: '', decimal_places: null, is_active: null },
      }),
    );
  });

  it('base_currency_cannot_be_deactivated', async () => {
    renderAt('#/currencies');
    const user = userEvent.setup();

    await user.click((await rowOf('TWD')).getByRole('button', { name: '編輯' }));
    const dialog = within(await screen.findByRole('dialog'));
    expect(dialog.getByRole('switch')).toBeDisabled();
    expect(dialog.getByText('本位幣不可停用')).toBeInTheDocument();
  });

  it('conflict_on_edit_prompts_reload', async () => {
    const invoke = renderAt('#/currencies', ADMIN, {
      update_currency: () =>
        commandError({ kind: 'api', code: 'CONFLICT', message: '此資料已被其他人修改' }),
    });
    const user = userEvent.setup();

    await user.click((await rowOf('USD')).getByRole('button', { name: '編輯' }));
    const dialog = within(await screen.findByRole('dialog'));
    await user.clear(dialog.getByLabelText('名稱'));
    await user.type(dialog.getByLabelText('名稱'), '美金');
    await user.click(save(dialog));

    expect(await dialog.findByText(CONFLICT_TITLE)).toBeInTheDocument();
    // 衝突後不可再次送出，必須重新載入
    expect(save(dialog)).toBeDisabled();
    const before = calls(invoke, 'list_currencies').length;
    await user.click(dialog.getByRole('button', { name: /重新載入/ }));

    await waitFor(() => expect(calls(invoke, 'list_currencies').length).toBeGreaterThan(before));
  });

  it('deactivate_from_list', async () => {
    const invoke = renderAt('#/currencies', ADMIN, {
      update_currency: () => currencyRow({ is_active: false, version: 3 }),
    });
    const user = userEvent.setup();

    await user.click((await rowOf('USD')).getByRole('button', { name: '停用' }));
    const confirm = await screen.findByRole('tooltip');
    await user.click(within(confirm).getByRole('button', { name: /停\s*用/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('update_currency', {
        code: 'USD',
        req: { version: 2, name: null, symbol: null, decimal_places: null, is_active: false },
      }),
    );
    expect(await screen.findByText('已停用幣別 USD')).toBeInTheDocument();
  });

  it('conflict_on_deactivate_shows_reload_alert', async () => {
    const invoke = renderAt('#/currencies', ADMIN, {
      update_currency: () =>
        commandError({ kind: 'api', code: 'CONFLICT', message: '此資料已被其他人修改' }),
    });
    const user = userEvent.setup();

    await user.click((await rowOf('USD')).getByRole('button', { name: '停用' }));
    const confirm = await screen.findByRole('tooltip');
    await user.click(within(confirm).getByRole('button', { name: /停\s*用/ }));

    expect(await screen.findByText(CONFLICT_TITLE)).toBeInTheDocument();
    const before = calls(invoke, 'list_currencies').length;
    await user.click(screen.getByRole('button', { name: /重新載入/ }));
    await waitFor(() => expect(screen.queryByText(CONFLICT_TITLE)).not.toBeInTheDocument());
    await waitFor(() => expect(calls(invoke, 'list_currencies').length).toBeGreaterThan(before));
  });
});
