import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import { CONFLICT_TITLE } from '../../components/ConflictAlert';
import type { CurrentUser, ExchangeRate } from '../../bindings';
import {
  ADMIN,
  CURRENCIES,
  EDITOR,
  VIEWER,
  exchangeRateRow,
  startupCommands,
} from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import { taipeiToday } from '../../utils/time';

const PAGE_LOAD_TIMEOUT = 5000;

const USD_RATE = exchangeRateRow();
const JPY_RATE = exchangeRateRow({
  id: '0190a000-0000-7000-8000-0000000000e2',
  currency_code: 'JPY',
  rate_date: '2026-11-10',
  rate: '0.210000',
  version: 1,
});

function page(items: ExchangeRate[]) {
  return { items, total: items.length, page: 1, page_size: 50 };
}

function renderRates(user: CurrentUser, extra: Record<string, CommandHandler> = {}) {
  window.location.hash = '#/exchange-rates';
  const invoke = mockCommands({
    ...startupCommands(user),
    list_currencies: () => CURRENCIES,
    list_exchange_rates: () => page([USD_RATE, JPY_RATE]),
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

function calls(invoke: ReturnType<typeof mockCommands>, command: string) {
  return invoke.mock.calls.filter(([cmd]) => cmd === command).map(([, args]) => args);
}

async function rowOf(date: string) {
  const cell = await screen.findByText(date, { selector: 'td' }, { timeout: PAGE_LOAD_TIMEOUT });
  return within(cell.closest('tr')!);
}

function save(dialog: ReturnType<typeof within>) {
  return dialog.getByRole('button', { name: /儲\s*存/ });
}

async function openCreate() {
  const user = userEvent.setup();
  await rowOf('2026-11-17');
  await user.click(screen.getByRole('button', { name: /新增匯率/ }));
  const dialog = within(await screen.findByRole('dialog'));
  return { user, dialog };
}

describe('匯率管理頁', () => {
  it('viewer_can_only_view', async () => {
    renderRates(VIEWER);

    const usd = await rowOf('2026-11-17');
    expect(usd.getByText('USD 美元')).toBeInTheDocument();
    // 匯率以伺服器的字串原樣顯示
    expect(usd.getByText('32.150000')).toBeInTheDocument();
    expect(usd.getByText('2026-11-17 09:30:00')).toBeInTheDocument();
    expect((await rowOf('2026-11-10')).getByText('已停用')).toBeInTheDocument();

    expect(screen.queryByRole('button', { name: /新增匯率/ })).not.toBeInTheDocument();
    expect(usd.queryByRole('button', { name: '修改' })).not.toBeInTheDocument();
    expect(usd.queryByRole('button', { name: '刪除' })).not.toBeInTheDocument();
    expect(screen.queryByText('操作', { selector: 'th' })).not.toBeInTheDocument();
  });

  it.each([
    ['editor', EDITOR],
    ['admin', ADMIN],
  ])('%s_can_edit', async (_, user) => {
    renderRates(user);

    const usd = await rowOf('2026-11-17');
    expect(screen.getByRole('button', { name: /新增匯率/ })).toBeInTheDocument();
    expect(usd.getByRole('button', { name: '修改' })).toBeEnabled();
    expect(usd.getByRole('button', { name: '刪除' })).toBeInTheDocument();
    // 幣別已停用時不可修改匯率，但可以刪除
    const jpy = await rowOf('2026-11-10');
    expect(jpy.getByRole('button', { name: '修改' })).toBeDisabled();
    expect(jpy.getByRole('button', { name: '刪除' })).toBeEnabled();
  });

  it('filters_by_currency', async () => {
    const invoke = renderRates(VIEWER);
    const user = userEvent.setup();
    await rowOf('2026-11-17');

    await user.click(screen.getByLabelText('幣別'));
    // 篩選選項包含已停用的外幣，不含本位幣
    expect(await screen.findByTitle('JPY 日圓（已停用）')).toBeInTheDocument();
    expect(screen.queryByTitle('TWD 新台幣')).not.toBeInTheDocument();
    await user.click(screen.getByTitle('USD 美元'));
    await user.click(screen.getByRole('button', { name: /查\s*詢/ }));

    await waitFor(() =>
      expect(calls(invoke, 'list_exchange_rates')).toContainEqual({
        query: { currency_code: 'USD', date_from: null, date_to: null, page: 1, page_size: 50 },
      }),
    );
  });

  it('creates_rate_as_string', async () => {
    const invoke = renderRates(EDITOR, {
      create_exchange_rate: () => exchangeRateRow({ rate: '32.123456' }),
    });
    const { user, dialog } = await openCreate();

    await user.click(dialog.getByLabelText('幣別'));
    // 只能選啟用中的外幣
    expect(screen.queryByTitle('JPY 日圓（已停用）', { exact: false })).toBeNull();
    await user.click(await screen.findByTitle('USD 美元'));
    // 日期預設為台北時間今天
    expect(dialog.getByLabelText('日期')).toHaveValue(taipeiToday());
    await user.type(dialog.getByLabelText('匯率'), '32.123456');
    await user.click(save(dialog));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('create_exchange_rate', {
        req: { currency_code: 'USD', rate_date: taipeiToday(), rate: '32.123456' },
      }),
    );
    expect(await screen.findByText('已新增匯率')).toBeInTheDocument();
  });

  it('invalid_rate_is_rejected_before_sending', async () => {
    const invoke = renderRates(EDITOR);
    const { user, dialog } = await openCreate();

    await user.click(dialog.getByLabelText('幣別'));
    await user.click(await screen.findByTitle('USD 美元'));
    await user.type(dialog.getByLabelText('匯率'), '32.1234567');
    await user.click(save(dialog));

    expect(await dialog.findByText('匯率最多 6 位小數')).toBeInTheDocument();
    await user.clear(dialog.getByLabelText('匯率'));
    await user.type(dialog.getByLabelText('匯率'), '0');
    expect(await dialog.findByText('匯率必須大於 0')).toBeInTheDocument();
    expect(calls(invoke, 'create_exchange_rate')).toEqual([]);
  });

  it('duplicate_date_error_is_shown_on_date_field', async () => {
    renderRates(EDITOR, {
      create_exchange_rate: () =>
        commandError({
          kind: 'api',
          code: 'VALIDATION',
          message: 'USD 在 2026-11-17 已有匯率',
          details: { field: 'rate_date' },
        }),
    });
    const { user, dialog } = await openCreate();

    await user.click(dialog.getByLabelText('幣別'));
    await user.click(await screen.findByTitle('USD 美元'));
    await user.type(dialog.getByLabelText('匯率'), '32.15');
    await user.click(save(dialog));

    const error = await dialog.findByText('USD 在 2026-11-17 已有匯率');
    expect(error.closest('.ant-form-item')).toContainElement(dialog.getByLabelText('日期'));
  });

  it('edit_only_changes_rate', async () => {
    const invoke = renderRates(EDITOR, {
      update_exchange_rate: () => exchangeRateRow({ rate: '32.200000', version: 5 }),
    });
    const user = userEvent.setup();

    await user.click((await rowOf('2026-11-17')).getByRole('button', { name: '修改' }));
    const dialog = within(await screen.findByRole('dialog'));
    expect(dialog.getByLabelText('日期')).toBeDisabled();
    expect(dialog.getByLabelText('匯率')).toHaveValue('32.150000');
    await user.clear(dialog.getByLabelText('匯率'));
    await user.type(dialog.getByLabelText('匯率'), '32.2');
    await user.click(save(dialog));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('update_exchange_rate', {
        id: USD_RATE.id,
        req: { version: 4, rate: '32.2' },
      }),
    );
  });

  it('unchanged_rate_is_not_sent', async () => {
    const invoke = renderRates(EDITOR);
    const user = userEvent.setup();

    await user.click((await rowOf('2026-11-17')).getByRole('button', { name: '修改' }));
    const dialog = within(await screen.findByRole('dialog'));
    await user.clear(dialog.getByLabelText('匯率'));
    await user.type(dialog.getByLabelText('匯率'), '32.15');
    await user.click(save(dialog));

    expect(await screen.findByText('沒有需要儲存的修改')).toBeInTheDocument();
    expect(calls(invoke, 'update_exchange_rate')).toEqual([]);
  });

  it('conflict_on_edit_prompts_reload', async () => {
    const invoke = renderRates(EDITOR, {
      update_exchange_rate: () =>
        commandError({ kind: 'api', code: 'CONFLICT', message: '此資料已被其他人修改' }),
    });
    const user = userEvent.setup();

    await user.click((await rowOf('2026-11-17')).getByRole('button', { name: '修改' }));
    const dialog = within(await screen.findByRole('dialog'));
    await user.clear(dialog.getByLabelText('匯率'));
    await user.type(dialog.getByLabelText('匯率'), '33');
    await user.click(save(dialog));

    expect(await dialog.findByText(CONFLICT_TITLE)).toBeInTheDocument();
    expect(save(dialog)).toBeDisabled();
    const before = calls(invoke, 'list_exchange_rates').length;
    await user.click(dialog.getByRole('button', { name: /重新載入/ }));
    await waitFor(() =>
      expect(calls(invoke, 'list_exchange_rates').length).toBeGreaterThan(before),
    );
  });

  it('deletes_with_version', async () => {
    const invoke = renderRates(EDITOR, { delete_exchange_rate: () => null });
    const user = userEvent.setup();

    await user.click((await rowOf('2026-11-17')).getByRole('button', { name: '刪除' }));
    const confirm = await screen.findByRole('tooltip');
    await user.click(within(confirm).getByRole('button', { name: /刪\s*除/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('delete_exchange_rate', { id: USD_RATE.id, version: 4 }),
    );
    expect(await screen.findByText('已刪除匯率')).toBeInTheDocument();
  });

  it('conflict_on_delete_shows_reload_alert', async () => {
    const invoke = renderRates(EDITOR, {
      delete_exchange_rate: () =>
        commandError({ kind: 'api', code: 'CONFLICT', message: '此資料已被其他人修改' }),
    });
    const user = userEvent.setup();

    await user.click((await rowOf('2026-11-17')).getByRole('button', { name: '刪除' }));
    const confirm = await screen.findByRole('tooltip');
    await user.click(within(confirm).getByRole('button', { name: /刪\s*除/ }));

    expect(await screen.findByText(CONFLICT_TITLE)).toBeInTheDocument();
    const before = calls(invoke, 'list_exchange_rates').length;
    await user.click(screen.getByRole('button', { name: /重新載入/ }));
    await waitFor(() => expect(screen.queryByText(CONFLICT_TITLE)).not.toBeInTheDocument());
    expect(calls(invoke, 'list_exchange_rates').length).toBeGreaterThan(before);
  });
});
