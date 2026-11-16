import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type {
  DefaultViewLayout_Serialize as DefaultViewLayout,
  EntityDefinition,
  FieldDefinition,
  FormulaPreviewRequest_Deserialize as FormulaPreviewRequest,
  FormulaPreview_Serialize as FormulaPreview,
  Record as DataRecord,
} from '../../bindings';
import { FORMULA_PENDING_TEXT, formulaErrorLabel } from '../../records/display';
import { ADMIN, startupCommands } from '../../test/fixtures';
import { mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import { REPAIR, fieldDef } from '../../test/schemaFixtures';

const PAGE_LOAD_TIMEOUT = 5000;

const PARTS: EntityDefinition = {
  ...REPAIR,
  id: '0196f5a0-0000-7000-8000-000000000110',
  key: 'parts',
  label: '零件',
  kind: 'detail',
  parent_entity_id: REPAIR.id,
  sort_order: 1,
};

function formula(
  patch: Partial<FieldDefinition> & Pick<FieldDefinition, 'field_key' | 'label'>,
): FieldDefinition {
  return fieldDef({
    data_type: 'formula',
    formula_result_type: 'money',
    decimal_places: 2,
    ...patch,
  });
}

const NAME = fieldDef({ field_key: 'name', label: '名稱', sort_order: 0 });
const A = fieldDef({ field_key: 'a', label: '被除數', data_type: 'integer', sort_order: 1 });
const B = fieldDef({ field_key: 'b', label: '除數', data_type: 'integer', sort_order: 2 });
const RATIO = formula({
  field_key: 'ratio',
  label: '比率',
  formula: '{a} / {b}',
  formula_result_type: 'decimal',
  sort_order: 3,
});
const TOTAL = formula({
  field_key: 'total',
  label: '合計',
  formula: 'SUM(parts.amount)',
  sort_order: 4,
});
const DISCOUNT = formula({
  field_key: 'discount',
  label: '折扣',
  formula: '{total} * {customer.rate}',
  sort_order: 5,
});
const QTY = fieldDef({
  entity_id: PARTS.id,
  field_key: 'qty',
  label: '數量',
  data_type: 'integer',
  default_value: 1,
  sort_order: 0,
});
const PRICE = fieldDef({
  entity_id: PARTS.id,
  field_key: 'price',
  label: '單價',
  data_type: 'money',
  decimal_places: 2,
  sort_order: 1,
});
const AMOUNT = formula({
  entity_id: PARTS.id,
  field_key: 'amount',
  label: '金額',
  formula: '{qty} * {price}',
  sort_order: 2,
});

const RECORD_ID = '0196f5a0-0000-7000-8000-000000000201';
const LINE_A = '0196f5a0-0000-7000-8000-000000000211';
const LINE_B = '0196f5a0-0000-7000-8000-000000000212';

function record(patch: Partial<DataRecord> = {}): DataRecord {
  const base = {
    record_no: null,
    title: '',
    lines: null,
    created_at: '2026-11-13T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-13T01:00:00Z',
    updated_by: null,
    version: 1,
  };
  return {
    ...base,
    id: RECORD_ID,
    title: '冷氣維修',
    data: { name: '冷氣維修', total: '250.00', discount: '25.00' },
    lines: {
      parts: [
        { ...base, id: LINE_A, data: { qty: 2, price: '100.00', amount: '200.00' } },
        { ...base, id: LINE_B, data: { qty: 1, price: '50.00', amount: '50.00' } },
      ],
    },
    version: 3,
    ...patch,
  };
}

/** 金額（以分計算，避免測試本身用浮點數）轉成兩位小數的字串。 */
function cents(n: number): string {
  return `${Math.trunc(n / 100)}.${String(Math.abs(n) % 100).padStart(2, '0')}`;
}

function toCents(v: unknown): number | undefined {
  if (v === undefined || v === null || v === '') {
    return undefined;
  }
  const [int = '0', frac = ''] = String(v).split('.');
  return Number(int) * 100 + Number(frac.padEnd(2, '0').slice(0, 2));
}

/**
 * 模擬 `preview_formulas`（實際的計算由 Rust 測試驗證，見 `src-tauri/src/formula.rs`）：
 * 明細 `amount = qty * price`、主表 `total = SUM(parts.amount)`、`ratio = a / b`；
 * `discount` 使用關聯引用。
 */
const fakePreview: CommandHandler = (args) => {
  const req = args.req as FormulaPreviewRequest;
  const lines = (req.lines?.parts ?? []).map((row) => {
    const price = toCents(row.price);
    const qty = row.qty === undefined ? undefined : Number(row.qty);
    return {
      values: { amount: price !== undefined && qty !== undefined ? cents(price * qty) : null },
      errors: {},
    };
  });
  const total = lines.reduce((sum, l) => sum + (toCents(l.values.amount) ?? 0), 0);
  const values: Record<string, unknown> = { total: cents(total), discount: null };
  const errors: Record<string, string> = { discount: '找不到欄位「customer.rate」' };
  if (req.data.a !== undefined && req.data.b !== undefined) {
    if (req.data.b === 0) {
      values.ratio = null;
      errors.ratio = '除數不可為零';
    } else {
      values.ratio = String(Number(req.data.a) / Number(req.data.b));
    }
  }
  const preview: FormulaPreview = {
    master: { values, errors },
    lines: { parts: lines },
    relation_dependent: [{ detail: null, field_key: 'discount' }],
  };
  return preview;
};

function renderForm(
  hash: string,
  masterFields: FieldDefinition[],
  extra: Record<string, CommandHandler> = {},
): ReturnType<typeof mockCommands> {
  window.location.hash = hash;
  const invoke = mockCommands({
    ...startupCommands(ADMIN),
    list_entities: () => [REPAIR, PARTS],
    get_entity: ({ key }) =>
      key === 'parts'
        ? { entity: PARTS, fields: [QTY, PRICE, AMOUNT] }
        : { entity: REPAIR, fields: masterFields },
    get_record: () => record(),
    preview_formulas: fakePreview,
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

function calls(invoke: ReturnType<typeof mockCommands>, cmd: string) {
  return invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args);
}

/** 主表公式欄位（唯讀）所在的 Form.Item。 */
function formulaItem(label: string): HTMLElement {
  return screen.getByTitle(label).closest<HTMLElement>('.ant-form-item')!;
}

function lineRows(): HTMLElement[] {
  const section = screen.getByRole('region', { name: '零件' });
  return Array.from(section.querySelectorAll<HTMLElement>('tbody tr.ant-table-row'));
}

const save = () => screen.getByRole('button', { name: /儲\s*存/ });

describe('公式欄位：表單即時預覽', () => {
  it('line_amount_and_master_total_update_when_qty_changes', async () => {
    const invoke = renderForm(`#/entities/repair/${RECORD_ID}`, [NAME, TOTAL]);
    const user = userEvent.setup();
    await screen.findByRole('textbox', { name: '名稱' }, { timeout: PAGE_LOAD_TIMEOUT });

    // 公式欄位唯讀：沒有輸入框，依結果型態格式化
    expect(within(lineRows()[0]!).queryByLabelText('金額')).not.toBeInTheDocument();
    await waitFor(() => expect(formulaItem('合計')).toHaveTextContent('250.00'));
    expect(lineRows()[0]).toHaveTextContent('200.00');

    const qty = within(lineRows()[0]!).getByLabelText('數量');
    await user.clear(qty);
    await user.type(qty, '3');

    await waitFor(() => expect(lineRows()[0]).toHaveTextContent('300.00'));
    await waitFor(() => expect(formulaItem('合計')).toHaveTextContent('350.00'));

    // 預覽的輸入為 JSON 存值；明細依表單中的順序
    const last = calls(invoke, 'preview_formulas').at(-1)!.req as FormulaPreviewRequest;
    expect(last.data).toEqual({ name: '冷氣維修' });
    expect(last.lines).toEqual({
      parts: [
        { qty: 3, price: '100.00' },
        { qty: 1, price: '50.00' },
      ],
    });
    expect(last.details?.map((d) => d.key)).toEqual(['parts']);

    // 新增明細列也會更新（新的列套用預設值 qty = 1）
    await user.click(screen.getByRole('button', { name: /新增零件/ }));
    await user.type(within(lineRows()[2]!).getByLabelText('單價'), '10');
    await waitFor(() => expect(formulaItem('合計')).toHaveTextContent('360.00'));
  });

  it('division_by_zero_shows_error_and_still_saves', async () => {
    const invoke = renderForm('#/entities/repair/new', [NAME, A, B, RATIO], {
      create_record: () => record({ lines: {} }),
    });
    const user = userEvent.setup();
    await screen.findByRole('textbox', { name: '名稱' }, { timeout: PAGE_LOAD_TIMEOUT });

    await user.type(screen.getByRole('textbox', { name: '被除數' }), '1');
    await user.type(screen.getByRole('textbox', { name: '除數' }), '0');

    const icon = await screen.findByLabelText(formulaErrorLabel('除數不可為零'));
    expect(formulaItem('比率')).toContainElement(icon);

    await user.click(save());
    await waitFor(() => expect(calls(invoke, 'create_record')).toHaveLength(1));
    // 公式欄位的值不送出（由伺服器計算）
    expect(calls(invoke, 'create_record')[0]!.req).toEqual({
      data: { name: null, a: 1, b: 0 },
      lines: { parts: [] },
    });
  });

  it('relation_dependent_formula_shows_saved_value_and_pending_tag', async () => {
    renderForm(`#/entities/repair/${RECORD_ID}`, [NAME, TOTAL, DISCOUNT]);
    const user = userEvent.setup();
    const name = await screen.findByRole(
      'textbox',
      { name: '名稱' },
      { timeout: PAGE_LOAD_TIMEOUT },
    );
    await waitFor(() => expect(formulaItem('合計')).toHaveTextContent('250.00'));

    // 預覽沒有關聯資料：顯示上一次存檔的值，不顯示預覽的錯誤；沒有修改時不標示
    expect(formulaItem('折扣')).toHaveTextContent('25.00');
    expect(screen.queryByText(FORMULA_PENDING_TEXT)).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/公式錯誤/)).not.toBeInTheDocument();

    await user.type(name, '！');
    expect(await within(formulaItem('折扣')).findByText(FORMULA_PENDING_TEXT)).toBeInTheDocument();
    expect(formulaItem('折扣')).toHaveTextContent('25.00');
  });

  it('no_preview_without_formula_fields', async () => {
    const invoke = renderForm(`#/entities/repair/${RECORD_ID}`, [NAME], {
      get_entity: ({ key }) =>
        key === 'parts'
          ? { entity: PARTS, fields: [QTY, PRICE] }
          : { entity: REPAIR, fields: [NAME] },
    });
    await screen.findByRole('textbox', { name: '名稱' }, { timeout: PAGE_LOAD_TIMEOUT });
    expect(calls(invoke, 'preview_formulas')).toHaveLength(0);
  });
});

const GENERATED: DefaultViewLayout = {
  source: 'generated',
  layout: null,
  config: {
    columns: ['name', 'ratio', 'total'].map((k) => ({ field_key: k, width: null })),
    sort: null,
    filters: [],
  },
};

describe('公式欄位：列表', () => {
  function renderList(records: DataRecord[]) {
    window.location.hash = '#/entities/repair';
    return mockCommands({
      ...startupCommands(ADMIN),
      list_entities: () => [REPAIR],
      get_entity: () => ({ entity: REPAIR, fields: [NAME, RATIO, TOTAL] }),
      get_default_layout: () => GENERATED,
      list_layouts: () => [],
      list_records: () => ({ items: records, total: records.length, page: 1, page_size: 50 }),
    });
  }

  it('formats_by_result_type_and_shows_error_icon', async () => {
    renderList([
      record({
        lines: null,
        data: { name: '冷氣維修', total: '1234.5' },
        formula_errors: { ratio: '除數不可為零' },
      }),
    ]);
    renderWithProviders(<App />);
    const cell = await screen.findByText('冷氣維修', { selector: 'td' }, { timeout: 5000 });
    const row = within(cell.closest('tr')!);

    // 金額依結果型態：小數位數補零、千分位，靠右
    const total = row.getByText('1,234.50');
    expect(total.closest('td')).toHaveClass('ant-table-cell');
    expect(total.closest('td')).toHaveStyle({ textAlign: 'right' });
    expect(row.getByLabelText(formulaErrorLabel('除數不可為零'))).toBeInTheDocument();
    // 沒有錯誤的欄位不顯示圖示
    expect(row.getAllByLabelText(/公式錯誤/)).toHaveLength(1);
  });

  it('formula_columns_are_sortable', async () => {
    renderList([record({ lines: null })]);
    renderWithProviders(<App />);
    await screen.findByText('冷氣維修', { selector: 'td' }, { timeout: 5000 });
    const th = Array.from(document.querySelectorAll('.ant-card thead th')).find(
      (el) => el.textContent === '合計',
    );
    expect(th).toHaveClass('ant-table-column-has-sorters');
  });
});
