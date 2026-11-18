import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type { Record as DataRecord } from '../../bindings';
import {
  ADJUSTMENT_ID,
  STOCK_ADJUSTMENT,
  STOCK_ADJUSTMENT_FIELDS,
  adjustment,
} from '../../test/documentFixtures';
import { ADMIN, EDITOR, VIEWER, startupCommands } from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';

const PAGE_LOAD_TIMEOUT = 5000;
const POST_BUTTON = /過\s*帳/;
const VOID_BUTTON = /作\s*廢/;

function render(hash: string, extra: Record<string, CommandHandler> = {}, user = ADMIN) {
  window.location.hash = hash;
  const invoke = mockCommands({
    ...startupCommands(user),
    list_entities: () => [STOCK_ADJUSTMENT],
    get_entity: () => ({ entity: STOCK_ADJUSTMENT, fields: STOCK_ADJUSTMENT_FIELDS }),
    get_default_layout: () => ({
      source: 'generated',
      layout: null,
      config: {
        columns: ['adjustment_date', 'adjustment_type', 'status'].map((k) => ({
          field_key: k,
          width: null,
        })),
        sort: null,
        filters: [],
      },
    }),
    list_layouts: () => [],
    get_record: () => adjustment('draft'),
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

function calls(invoke: ReturnType<typeof mockCommands>, cmd: string) {
  return invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args);
}

async function waitForForm() {
  return screen.findByRole('textbox', { name: '單號' }, { timeout: PAGE_LOAD_TIMEOUT });
}

/** 表單右上角的按鈕區（確認對話框關閉時有動畫，jsdom 中會留在畫面上，不能用 screen 查詢）。 */
function actions() {
  return within(document.querySelector<HTMLElement>('.ant-card-extra')!);
}

/** 確認對話框（antd Modal.confirm）。 */
async function confirmDialog() {
  // 前一個對話框關閉時的動畫在 jsdom 不會結束，取最新開啟的
  const dialogs = await screen.findAllByRole('dialog');
  return within(dialogs.at(-1)!);
}

describe('系統單據列表', () => {
  const rows: DataRecord[] = [
    adjustment('draft', { id: 'r1', record_no: 'ADJ202611-0001' }),
    adjustment('posted', { id: 'r2', record_no: 'ADJ202611-0002' }),
    adjustment('void', { id: 'r3', record_no: 'ADJ202611-0003' }),
  ];

  it('shows_record_no_status_tags_and_deletes_only_drafts', async () => {
    render('#/entities/stock_adjustment', {
      list_records: () => ({ items: rows, total: 3, page: 1, page_size: 50 }),
    });
    const draft = await screen.findByText('ADJ202611-0001', {}, { timeout: PAGE_LOAD_TIMEOUT });
    const row = (no: string) => within(screen.getByText(no).closest('tr')!);

    expect(within(draft.closest('tr')!).getByText('草稿')).toHaveClass('ant-tag-default');
    expect(row('ADJ202611-0002').getByText('已過帳')).toHaveClass('ant-tag-green');
    expect(row('ADJ202611-0003').getByText('已作廢')).toHaveClass('ant-tag-red');

    expect(row('ADJ202611-0001').getByRole('button', { name: /刪\s*除/ })).toBeInTheDocument();
    expect(row('ADJ202611-0002').queryByRole('button', { name: /刪\s*除/ })).toBeNull();
    expect(row('ADJ202611-0003').queryByRole('button', { name: /刪\s*除/ })).toBeNull();
  });

  it('filters_by_status', async () => {
    const invoke = render('#/entities/stock_adjustment', {
      list_records: () => ({ items: rows, total: 3, page: 1, page_size: 50 }),
    });
    const user = userEvent.setup();
    await screen.findByText('ADJ202611-0001', {}, { timeout: PAGE_LOAD_TIMEOUT });

    await user.click(screen.getByRole('combobox', { name: '狀態篩選' }));
    await user.click(await screen.findByTitle('已過帳'));

    await waitFor(() =>
      expect(calls(invoke, 'list_records').at(-1)?.query).toMatchObject({
        page: 1,
        filters: [{ field_key: 'status', op: 'eq', value: 'posted' }],
      }),
    );

    await user.click(screen.getByRole('combobox', { name: '狀態篩選' }));
    await user.click(await screen.findByTitle('全部狀態'));
    await waitFor(() =>
      expect(calls(invoke, 'list_records').at(-1)?.query).toMatchObject({ filters: [] }),
    );
  });
});

describe('系統單據表單', () => {
  it('post_then_void', async () => {
    let current = adjustment('draft');
    const invoke = render('#/entities/stock_adjustment/' + ADJUSTMENT_ID, {
      get_record: () => current,
      post_record: () => (current = adjustment('posted')),
      void_record: () => (current = adjustment('void')),
    });
    const user = userEvent.setup();
    await waitForForm();

    // 草稿：可以編輯，可以過帳，不能作廢
    expect(screen.getByText('草稿', { selector: '.ant-tag' })).toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: '備註' })).toBeEnabled();
    expect(actions().queryByRole('button', { name: VOID_BUTTON })).toBeNull();

    await user.click(actions().getByRole('button', { name: POST_BUTTON }));
    const dialog = await confirmDialog();
    expect(dialog.getAllByText('確定要過帳這張盤點調整單？').length).toBeGreaterThan(0);
    await user.click(dialog.getByRole('button', { name: POST_BUTTON }));

    await waitFor(() =>
      expect(calls(invoke, 'post_record')).toEqual([
        { key: 'stock_adjustment', id: ADJUSTMENT_ID, version: 1 },
      ]),
    );
    // 已過帳：唯讀，沒有儲存按鈕，可以作廢
    expect(await screen.findByText('已過帳', { selector: '.ant-tag' })).toBeInTheDocument();
    expect(screen.getByText(/此單據已過帳，不能修改。/)).toHaveTextContent(
      '過帳時間：2026-11-18 10:00:00',
    );
    expect(screen.getByRole('textbox', { name: '備註' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: /儲\s*存/ })).toBeNull();
    expect(actions().queryByRole('button', { name: POST_BUTTON })).toBeNull();

    await user.click(actions().getByRole('button', { name: VOID_BUTTON }));
    const voidDialog = await confirmDialog();
    await user.click(voidDialog.getByRole('button', { name: VOID_BUTTON }));
    await waitFor(() =>
      expect(calls(invoke, 'void_record')).toEqual([
        { key: 'stock_adjustment', id: ADJUSTMENT_ID, version: 2 },
      ]),
    );
    expect(await screen.findByText('已作廢', { selector: '.ant-tag' })).toBeInTheDocument();
    expect(actions().queryByRole('button', { name: VOID_BUTTON })).toBeNull();
    expect(actions().queryByRole('button', { name: POST_BUTTON })).toBeNull();
  });

  it('viewer_sees_no_action_buttons', async () => {
    render(
      '#/entities/stock_adjustment/' + ADJUSTMENT_ID,
      {
        get_record: () => adjustment('posted'),
      },
      VIEWER,
    );
    await waitForForm();
    expect(screen.getByText('已過帳', { selector: '.ant-tag' })).toBeInTheDocument();
    expect(actions().queryByRole('button', { name: VOID_BUTTON })).toBeNull();
    expect(actions().queryByRole('button', { name: POST_BUTTON })).toBeNull();
  });

  it('editor_without_post_permission_cannot_post', async () => {
    render(
      '#/entities/stock_adjustment/' + ADJUSTMENT_ID,
      {},
      { ...EDITOR, permissions: EDITOR.permissions.filter((p) => p !== 'record.post') },
    );
    await waitForForm();
    // 可以編輯草稿，但沒有過帳按鈕
    expect(screen.getByRole('button', { name: /儲\s*存/ })).toBeInTheDocument();
    expect(actions().queryByRole('button', { name: POST_BUTTON })).toBeNull();
  });

  it('unsaved_changes_block_posting', async () => {
    render('#/entities/stock_adjustment/' + ADJUSTMENT_ID);
    const user = userEvent.setup();
    await waitForForm();
    await user.type(screen.getByRole('textbox', { name: '備註' }), '改');
    expect(actions().getByRole('button', { name: POST_BUTTON })).toBeDisabled();
  });

  it('shows_stock_insufficient_details', async () => {
    render('#/entities/stock_adjustment/' + ADJUSTMENT_ID, {
      get_record: () => adjustment('posted'),
      void_record: () =>
        commandError({
          kind: 'api',
          code: 'STOCK_INSUFFICIENT',
          message: '庫存不足',
          details: {
            items: [
              {
                product_id: 'p1',
                product_code: 'A001',
                product_name: '螺絲',
                lot_id: 'l1',
                lot_no: 'L-01',
                required: '10.0000',
                available: '4.0000',
                shortage: '6.0000',
                lines: [{ detail_key: 'stock_adjustment_line', index: 0 }],
              },
            ],
          },
        }),
    });
    const user = userEvent.setup();
    await waitForForm();
    await user.click(actions().getByRole('button', { name: VOID_BUTTON }));
    await user.click((await confirmDialog()).getByRole('button', { name: VOID_BUTTON }));

    expect(await screen.findByText('庫存不足，無法作廢')).toBeInTheDocument();
    expect(
      screen.getByText('A001 螺絲（批號 L-01）：需要 10，目前可用 4，不足 6（明細第 1 列）'),
    ).toBeInTheDocument();
    // 單據維持原狀態
    expect(screen.getByText('已過帳', { selector: '.ant-tag' })).toBeInTheDocument();
  });

  it('conflict_prompts_reload', async () => {
    let current = adjustment('draft');
    const invoke = render('#/entities/stock_adjustment/' + ADJUSTMENT_ID, {
      get_record: () => current,
      post_record: () =>
        commandError({ kind: 'api', code: 'CONFLICT', message: '此資料已被其他人修改' }),
    });
    const user = userEvent.setup();
    await waitForForm();
    await user.click(actions().getByRole('button', { name: POST_BUTTON }));
    await user.click((await confirmDialog()).getByRole('button', { name: POST_BUTTON }));

    expect(await screen.findByText('資料已被其他人修改')).toBeInTheDocument();
    current = adjustment('posted');
    await user.click(screen.getByRole('button', { name: /重新載入/ }));
    expect(await screen.findByText('已過帳', { selector: '.ant-tag' })).toBeInTheDocument();
    expect(calls(invoke, 'get_record').length).toBeGreaterThanOrEqual(2);
  });
});
