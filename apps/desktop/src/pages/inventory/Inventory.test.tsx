import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type { Record as DataRecord } from '../../bindings';
import {
  ADJUSTMENT_ID,
  LOT_ID,
  PRODUCT,
  PRODUCT_ID,
  STOCK_ADJUSTMENT,
  STOCK_ADJUSTMENT_FIELDS,
  STOCK_ADJUSTMENT_LINE,
  STOCK_ADJUSTMENT_LINE_FIELDS,
  adjustment,
  lot,
} from '../../test/documentFixtures';
import { ADMIN, VIEWER, startupCommands } from '../../test/fixtures';
import { mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { navSectionItems } from '../../test/nav';
import { renderWithProviders } from '../../test/render';
import { taipeiToday } from '../../utils/time';

const PAGE_LOAD_TIMEOUT = 5000;

function balance(patch: Record<string, unknown> = {}) {
  return {
    product_id: PRODUCT_ID,
    code: 'A001',
    name: '不鏽鋼螺絲 M6',
    spec: '6mm',
    unit_name: '個',
    is_active: true,
    qty_on_hand: '35.5000',
    stock_value_twd: '443.75',
    safety_stock: '50.0000',
    below_safety_stock: true,
    ...patch,
  };
}

function txn(patch: Record<string, unknown> = {}) {
  return {
    id: '0196f5a0-0000-7000-8000-000000000801',
    txn_at: '2026-11-18T02:00:00Z',
    txn_date: '2026-11-18',
    txn_type: 'adjustment',
    product_id: PRODUCT_ID,
    product_code: 'A001',
    product_name: '不鏽鋼螺絲 M6',
    lot_id: LOT_ID,
    lot_no: 'L-20261101',
    qty_change: '100.0000',
    unit_cost_twd: '12.5000',
    ref_doc_type: 'stock_adjustment',
    ref_doc_id: ADJUSTMENT_ID,
    ref_record_no: 'ADJ202611-0001',
    ref_line_id: '0196f5a0-0000-7000-8000-000000000901',
    created_by: ADMIN.id,
    created_by_name: '王管理',
    ...patch,
  };
}

const page = <T,>(items: T[]) => ({ items, total: items.length, page: 1, page_size: 50 });

function render(hash: string, extra: Record<string, CommandHandler> = {}, user = ADMIN) {
  window.location.hash = hash;
  const invoke = mockCommands({
    ...startupCommands(user),
    list_entities: () => [PRODUCT, STOCK_ADJUSTMENT, STOCK_ADJUSTMENT_LINE],
    get_entity: ({ key }) =>
      key === 'stock_adjustment_line'
        ? { entity: STOCK_ADJUSTMENT_LINE, fields: STOCK_ADJUSTMENT_LINE_FIELDS }
        : { entity: STOCK_ADJUSTMENT, fields: STOCK_ADJUSTMENT_FIELDS },
    list_stock_balances: () =>
      page([
        balance(),
        balance({
          product_id: 'p2',
          code: 'B002',
          name: '螺帽',
          below_safety_stock: false,
          safety_stock: null,
        }),
      ]),
    list_product_lots: () => page([lot()]),
    list_inventory_transactions: () => page([txn()]),
    get_record: ({ key }) =>
      key === 'product'
        ? {
            ...adjustment(),
            id: PRODUCT_ID,
            title: 'A001 不鏽鋼螺絲 M6',
            record_no: null,
            data: { code: 'A001', name: '不鏽鋼螺絲 M6' },
          }
        : adjustment(),
    lookup_records: () => [{ id: PRODUCT_ID, title: 'A001 不鏽鋼螺絲 M6' }],
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

function calls(invoke: ReturnType<typeof mockCommands>, cmd: string) {
  return invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args);
}

describe('庫存選單', () => {
  it.each([ADMIN, VIEWER])('shows_inventory_group (%#)', async (user) => {
    render('#/stock', {}, user);
    // 資料類別載入後，盤點調整單排在庫存查詢之前
    const adjustmentItem = await screen.findByRole(
      'menuitem',
      { name: /盤點調整單/ },
      { timeout: PAGE_LOAD_TIMEOUT },
    );
    expect(adjustmentItem).toBeInTheDocument();
    // 目前頁面（庫存查詢）所屬的區塊自動展開
    expect(navSectionItems('庫存')).toEqual(['盤點調整單', '庫存查詢']);
  });
});

describe('庫存查詢', () => {
  it('lists_balances_and_marks_below_safety_stock', async () => {
    const invoke = render('#/stock');
    const user = userEvent.setup();
    const cell = await screen.findByText('不鏽鋼螺絲 M6', {}, { timeout: PAGE_LOAD_TIMEOUT });
    const row = within(cell.closest('tr')!);
    expect(row.getByText('35.5')).toBeInTheDocument();
    expect(row.getByText('50')).toBeInTheDocument();
    expect(row.getByText('443.75')).toBeInTheDocument();
    expect(row.getByText('低於安全庫存')).toBeInTheDocument();
    const other = within(screen.getByText('螺帽').closest('tr')!);
    expect(other.queryByText('低於安全庫存')).toBeNull();

    await user.click(screen.getByRole('checkbox', { name: '只顯示低於安全庫存' }));
    await waitFor(() =>
      expect(calls(invoke, 'list_stock_balances').at(-1)?.query).toMatchObject({
        below_safety_stock: 'true',
        page: 1,
      }),
    );
    await user.type(screen.getByRole('searchbox', { name: '料號或品名' }), '螺絲{Enter}');
    await waitFor(() =>
      expect(calls(invoke, 'list_stock_balances').at(-1)?.query).toMatchObject({ q: '螺絲' }),
    );

    await user.click(screen.getByText('不鏽鋼螺絲 M6'));
    await waitFor(() => expect(window.location.hash).toBe(`#/stock/${PRODUCT_ID}`));
  });

  it('product_lots_and_transactions_link_to_documents', async () => {
    const invoke = render(`#/stock/${PRODUCT_ID}`);
    const user = userEvent.setup();
    expect(
      await screen.findByText('A001 不鏽鋼螺絲 M6', {}, { timeout: PAGE_LOAD_TIMEOUT }),
    ).toBeInTheDocument();

    const lots = within(await screen.findByRole('table', { name: '批號' }));
    expect(await lots.findByText('L-20261101')).toBeInTheDocument();
    expect(lots.getByText('100')).toBeInTheDocument();
    expect(lots.getByText('35.5')).toBeInTheDocument();
    expect(lots.getByText('12.50')).toBeInTheDocument();
    expect(lots.getByRole('link', { name: 'ADJ202611-0001' })).toHaveAttribute(
      'href',
      `#/entities/stock_adjustment/${ADJUSTMENT_ID}`,
    );

    const txns = within(screen.getByRole('table', { name: '異動流水帳' }));
    expect(await txns.findByText('+100')).toBeInTheDocument();
    expect(txns.getByText('盤點調整')).toBeInTheDocument();
    expect(txns.getByText('王管理')).toBeInTheDocument();
    expect(txns.getByText('2026-11-18 10:00:00')).toBeInTheDocument();
    expect(calls(invoke, 'list_inventory_transactions')[0]?.query).toMatchObject({
      product_id: PRODUCT_ID,
      lot_id: null,
    });

    // 點批號只看該批號的異動
    await user.click(lots.getByText('L-20261101'));
    await waitFor(() =>
      expect(calls(invoke, 'list_inventory_transactions').at(-1)?.query).toMatchObject({
        lot_id: LOT_ID,
      }),
    );
    expect(screen.getByText('批號 L-20261101')).toBeInTheDocument();

    // 點來源單號開啟單據
    await user.click(txns.getByRole('link', { name: 'ADJ202611-0001' }));
    await waitFor(() =>
      expect(window.location.hash).toBe(`#/entities/stock_adjustment/${ADJUSTMENT_ID}`),
    );
  });
});

describe('盤點調整明細', () => {
  const LINE_ID = '0196f5a0-0000-7000-8000-000000000901';

  function withLine(data: Record<string, unknown>): DataRecord {
    return adjustment('draft', {
      lines: {
        stock_adjustment_line: [
          {
            ...adjustment(),
            id: LINE_ID,
            record_no: null,
            document: null,
            title: 'A001',
            data: { product_id: PRODUCT_ID, qty: '2.0000', reason: null, ...data },
            relations: {
              product_id: {
                id: PRODUCT_ID,
                title: 'A001 不鏽鋼螺絲 M6',
                deleted: false,
                inactive: false,
              },
            },
          },
        ],
      },
    });
  }

  const cell = (label: string) => screen.getAllByLabelText(label).at(-1)!;

  it('loss_selects_existing_lot_and_shows_current_qty', async () => {
    const invoke = render(`#/entities/stock_adjustment/${ADJUSTMENT_ID}`, {
      get_record: () => withLine({ line_type: 'loss', lot_no: 'L-20261101', unit_cost_twd: null }),
    });
    const user = userEvent.setup();
    const section = within(
      await screen.findByRole('region', { name: '盤點調整明細' }, { timeout: PAGE_LOAD_TIMEOUT }),
    );

    // 批號為選單，列出該商品有數量的批號；帶出目前數量；成本不可輸入
    expect(await section.findByText('35.5', { selector: 'strong' })).toBeInTheDocument();
    expect(calls(invoke, 'list_product_lots')[0]).toEqual({
      productId: PRODUCT_ID,
      query: { include_empty: null, page_size: 200 },
    });
    expect(section.getByRole('combobox', { name: '批號' })).toBeInTheDocument();
    expect(section.getByLabelText('單位成本')).toBeDisabled();

    // 改成盤盈新批號：批號清空並改為自行輸入，成本可以輸入
    await user.click(cell('調整方式'));
    await user.click((await screen.findAllByTitle('盤盈：新批號')).at(-1)!);
    const lotInput = await section.findByRole('textbox', { name: '批號' });
    expect(lotInput).toHaveValue('');
    expect(lotInput).toHaveAttribute('placeholder', '留空則過帳時自動產生');
    expect(section.getByLabelText('單位成本')).toBeEnabled();
    expect(section.getByText('新批號')).toBeInTheDocument();
  });

  it('gain_existing_lists_empty_lots_too', async () => {
    const invoke = render(`#/entities/stock_adjustment/${ADJUSTMENT_ID}`, {
      get_record: () =>
        withLine({ line_type: 'gain_existing', lot_no: 'L-20261101', unit_cost_twd: null }),
    });
    await screen.findByRole('region', { name: '盤點調整明細' }, { timeout: PAGE_LOAD_TIMEOUT });
    await waitFor(() =>
      expect(calls(invoke, 'list_product_lots')[0]).toMatchObject({
        query: { include_empty: 'true' },
      }),
    );
  });

  it('opening_line_keeps_saved_values', async () => {
    render(`#/entities/stock_adjustment/${ADJUSTMENT_ID}`, {
      get_record: () =>
        withLine({ line_type: 'gain_new', lot_no: 'OPEN-01', unit_cost_twd: '12.5000' }),
    });
    const section = within(
      await screen.findByRole('region', { name: '盤點調整明細' }, { timeout: PAGE_LOAD_TIMEOUT }),
    );
    // 開啟既有資料時不會清掉已存檔的批號與成本
    expect(await section.findByRole('textbox', { name: '批號' })).toHaveValue('OPEN-01');
    expect(section.getByLabelText('單位成本')).toHaveValue('12.5000');
    expect(screen.queryByText('未存檔')).toBeNull();
  });

  it('new_adjustment_defaults_to_today', async () => {
    render('#/entities/stock_adjustment/new');
    const date = await screen.findByLabelText('調整日期', {}, { timeout: PAGE_LOAD_TIMEOUT });
    expect(date).toHaveValue(taipeiToday());
  });
});
