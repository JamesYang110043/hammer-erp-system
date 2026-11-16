import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type { EntityDefinition, Record as DataRecord } from '../../bindings';
import { ADMIN, VIEWER, startupCommands } from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import { REPAIR, fieldDef } from '../../test/schemaFixtures';

const PAGE_LOAD_TIMEOUT = 5000;

/** 有單號規則的單據。 */
const ORDER: EntityDefinition = {
  ...REPAIR,
  kind: 'document',
  numbering_pattern: 'R{YYYYMM}-{####}',
};

/** 維修單的明細子表。 */
const PARTS: EntityDefinition = {
  ...REPAIR,
  id: '0196f5a0-0000-7000-8000-000000000110',
  key: 'parts',
  label: '零件',
  kind: 'detail',
  parent_entity_id: REPAIR.id,
  sort_order: 1,
};

/** 另一個主表的明細子表（不應該出現）。 */
const OTHER_DETAIL: EntityDefinition = {
  ...PARTS,
  id: '0196f5a0-0000-7000-8000-000000000111',
  key: 'other_lines',
  label: '其他明細',
  parent_entity_id: '0196f5a0-0000-7000-8000-000000000999',
};

const NAME = fieldDef({ field_key: 'name', label: '名稱', is_required: true });
const PART = fieldDef({
  entity_id: PARTS.id,
  field_key: 'part',
  label: '品名',
  is_required: true,
  sort_order: 0,
});
const QTY = fieldDef({
  entity_id: PARTS.id,
  field_key: 'qty',
  label: '數量',
  data_type: 'integer',
  default_value: 1,
  sort_order: 1,
});

const RECORD_ID = '0196f5a0-0000-7000-8000-000000000201';
const LINE_A = '0196f5a0-0000-7000-8000-000000000211';
const LINE_B = '0196f5a0-0000-7000-8000-000000000212';

function line(id: string, data: Record<string, unknown>): DataRecord {
  return {
    id,
    record_no: null,
    title: String(data.part ?? ''),
    data,
    lines: null,
    created_at: '2026-11-13T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-13T01:00:00Z',
    updated_by: null,
    version: 1,
  };
}

function record(patch: Partial<DataRecord> = {}): DataRecord {
  return {
    id: RECORD_ID,
    record_no: 'R202611-0001',
    title: 'R202611-0001',
    data: { name: '冷氣維修' },
    lines: {
      parts: [line(LINE_A, { part: '濾網', qty: 2 }), line(LINE_B, { part: '冷媒', qty: 1 })],
    },
    created_at: '2026-11-13T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-13T01:00:00Z',
    updated_by: null,
    version: 3,
    ...patch,
  };
}

function renderAt(
  hash: string,
  extra: Record<string, CommandHandler> = {},
  user = ADMIN,
): ReturnType<typeof mockCommands> {
  window.location.hash = hash;
  const invoke = mockCommands({
    ...startupCommands(user),
    list_entities: () => [ORDER, PARTS, OTHER_DETAIL],
    get_entity: ({ key }) =>
      key === 'parts'
        ? { entity: PARTS, fields: [QTY, PART] }
        : key === 'other_lines'
          ? { entity: OTHER_DETAIL, fields: [] }
          : { entity: ORDER, fields: [NAME] },
    get_record: () => record(),
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

async function waitForForm() {
  return screen.findByRole('textbox', { name: '名稱' }, { timeout: PAGE_LOAD_TIMEOUT });
}

function calls(invoke: ReturnType<typeof mockCommands>, cmd: string) {
  return invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args);
}

/** 明細表格的資料列。 */
function lineRows(): HTMLElement[] {
  const section = screen.getByRole('region', { name: '零件' });
  return Array.from(section.querySelectorAll<HTMLElement>('tbody tr.ant-table-row'));
}

/** 第 `row` 列（從 0 開始）的儲存格輸入框。 */
function cell(row: number, label: string): HTMLElement {
  return within(lineRows()[row]!).getByLabelText(label);
}

/** 儲存格所在的 Form.Item（錯誤訊息顯示的位置）。 */
function cellItem(row: number, label: string): HTMLElement {
  return cell(row, label).closest<HTMLElement>('.ant-form-item')!;
}

const save = () => screen.getByRole('button', { name: /儲\s*存/ });

describe('明細子表', () => {
  it('shows_only_own_detail_tables_with_lines_in_order', async () => {
    renderAt(`#/entities/repair/${RECORD_ID}`);
    await waitForForm();

    expect(screen.getByRole('region', { name: '零件' })).toBeInTheDocument();
    expect(screen.queryByRole('region', { name: '其他明細' })).not.toBeInTheDocument();
    expect(lineRows()).toHaveLength(2);
    expect(cell(0, '品名')).toHaveValue('濾網');
    expect(cell(0, '數量')).toHaveValue('2');
    expect(cell(1, '品名')).toHaveValue('冷媒');
  });

  it('create_sends_lines_with_new_rows', async () => {
    const invoke = renderAt('#/entities/repair/new', {
      create_record: () => record({ version: 1 }),
    });
    const user = userEvent.setup();
    await waitForForm();

    expect(lineRows()).toHaveLength(0);
    await user.type(screen.getByRole('textbox', { name: '名稱' }), '冷氣維修');
    await user.click(screen.getByRole('button', { name: /新增零件/ }));
    await user.click(screen.getByRole('button', { name: /新增零件/ }));
    // 新的列套用欄位預設值
    expect(cell(0, '數量')).toHaveValue('1');
    await user.type(cell(0, '品名'), '濾網');
    await user.clear(cell(0, '數量'));
    await user.type(cell(0, '數量'), '3');
    await user.type(cell(1, '品名'), '冷媒');
    await user.click(save());

    await waitFor(() => expect(calls(invoke, 'create_record')).toHaveLength(1));
    expect(calls(invoke, 'create_record')[0]).toEqual({
      key: 'repair',
      req: {
        data: { name: '冷氣維修' },
        lines: {
          parts: [{ data: { part: '濾網', qty: 3 } }, { data: { part: '冷媒', qty: 1 } }],
        },
      },
    });
  });

  it('line_cells_are_validated_before_submit', async () => {
    const invoke = renderAt('#/entities/repair/new', { create_record: () => record() });
    const user = userEvent.setup();
    await waitForForm();

    await user.type(screen.getByRole('textbox', { name: '名稱' }), '冷氣維修');
    await user.click(screen.getByRole('button', { name: /新增零件/ }));
    await user.clear(cell(0, '數量'));
    await user.type(cell(0, '數量'), '1.5');
    await user.click(save());

    expect(await within(cellItem(0, '品名')).findByText('此欄位為必填')).toBeInTheDocument();
    expect(within(cellItem(0, '數量')).getByText('必須是整數')).toBeInTheDocument();
    expect(calls(invoke, 'create_record')).toHaveLength(0);
  });

  it('update_sends_whole_detail_table_with_changed_fields', async () => {
    const invoke = renderAt(`#/entities/repair/${RECORD_ID}`, {
      update_record: () => record({ version: 4 }),
    });
    const user = userEvent.setup();
    await waitForForm();
    expect(save()).toBeDisabled();

    // 修改第 1 列、刪除第 2 列、新增一列
    await user.clear(cell(0, '數量'));
    await user.type(cell(0, '數量'), '5');
    await user.click(screen.getByRole('button', { name: '刪除第 2 列' }));
    await user.click(screen.getByRole('button', { name: /新增零件/ }));
    await user.type(cell(1, '品名'), '銅管');
    await user.click(save());

    await waitFor(() =>
      expect(calls(invoke, 'update_record')[0]).toEqual({
        key: 'repair',
        id: RECORD_ID,
        req: {
          version: 3,
          data: {},
          lines: {
            parts: [{ id: LINE_A, data: { qty: 5 } }, { data: { part: '銅管', qty: 1 } }],
          },
        },
      }),
    );
  });

  it('deleting_a_line_alone_marks_dirty_and_sends_remaining_rows', async () => {
    const invoke = renderAt(`#/entities/repair/${RECORD_ID}`, {
      update_record: () => record({ version: 4 }),
    });
    const user = userEvent.setup();
    await waitForForm();

    await user.click(screen.getByRole('button', { name: '刪除第 1 列' }));
    expect(screen.getByText('未存檔')).toBeInTheDocument();
    expect(lineRows()).toHaveLength(1);
    await user.click(save());

    await waitFor(() =>
      expect(calls(invoke, 'update_record')[0]).toEqual({
        key: 'repair',
        id: RECORD_ID,
        req: { version: 3, data: {}, lines: { parts: [{ id: LINE_B, data: {} }] } },
      }),
    );
  });

  it('update_without_line_changes_omits_lines', async () => {
    const invoke = renderAt(`#/entities/repair/${RECORD_ID}`, {
      update_record: () => record({ version: 4 }),
    });
    const user = userEvent.setup();
    const name = await waitForForm();

    await user.type(name, '（急）');
    await user.click(save());

    await waitFor(() =>
      expect(calls(invoke, 'update_record')[0]).toEqual({
        key: 'repair',
        id: RECORD_ID,
        req: { version: 3, data: { name: '冷氣維修（急）' } },
      }),
    );
  });

  it('server_line_errors_are_shown_on_cells', async () => {
    renderAt(`#/entities/repair/${RECORD_ID}`, {
      update_record: () =>
        commandError({
          kind: 'api',
          code: 'VALIDATION',
          message: '資料有誤',
          details: {
            fields: [],
            lines: [
              {
                detail_key: 'parts',
                index: 1,
                field_key: 'part',
                message: '「冷媒」已存在，不可重複',
              },
              { detail_key: 'parts', index: 0, field_key: 'qty', message: '不可大於 3' },
              { detail_key: 'parts', index: 1, field_key: null, message: '明細不存在' },
            ],
          },
        }),
    });
    const user = userEvent.setup();
    await waitForForm();

    await user.clear(cell(0, '數量'));
    await user.type(cell(0, '數量'), '9');
    await user.click(save());

    expect(
      await within(cellItem(1, '品名')).findByText('「冷媒」已存在，不可重複'),
    ).toBeInTheDocument();
    expect(within(cellItem(0, '數量')).getByText('不可大於 3')).toBeInTheDocument();
    expect(within(cellItem(0, '品名')).queryByText('「冷媒」已存在，不可重複')).toBeNull();
    // 不屬於儲存格的錯誤以訊息顯示
    expect(await screen.findByText('零件 第 2 列：明細不存在')).toBeInTheDocument();
  });

  it('conflict_reuses_reload_prompt', async () => {
    renderAt(`#/entities/repair/${RECORD_ID}`, {
      update_record: () =>
        commandError({ kind: 'api', code: 'CONFLICT', message: '版本不符', details: null }),
    });
    const user = userEvent.setup();
    await waitForForm();

    await user.click(screen.getByRole('button', { name: '刪除第 1 列' }));
    await user.click(save());

    expect(await screen.findByText('資料已被其他人修改')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /重新載入/ })).toBeInTheDocument();
  });

  it('viewer_cannot_add_or_delete_lines', async () => {
    renderAt(`#/entities/repair/${RECORD_ID}`, {}, VIEWER);
    await waitForForm();

    expect(cell(0, '品名')).toBeDisabled();
    expect(screen.queryByRole('button', { name: /新增零件/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: '刪除第 1 列' })).not.toBeInTheDocument();
  });
});

describe('單號', () => {
  it('form_shows_record_no_readonly', async () => {
    renderAt(`#/entities/repair/${RECORD_ID}`);
    await waitForForm();

    const no = screen.getByRole('textbox', { name: '單號' });
    expect(no).toHaveValue('R202611-0001');
    expect(no).toBeDisabled();
  });

  it('new_form_shows_generated_after_save', async () => {
    renderAt('#/entities/repair/new');
    await waitForForm();

    const no = screen.getByRole('textbox', { name: '單號' });
    expect(no).toHaveValue('');
    expect(no).toHaveAttribute('placeholder', '存檔後產生');
  });

  it('form_without_numbering_hides_record_no', async () => {
    renderAt('#/entities/repair/new', {
      list_entities: () => [REPAIR],
      get_entity: () => ({ entity: REPAIR, fields: [NAME] }),
    });
    await waitForForm();

    expect(screen.queryByRole('textbox', { name: '單號' })).not.toBeInTheDocument();
  });

  it('list_shows_record_no_column_and_sorts_by_it', async () => {
    const invoke = renderAt('#/entities/repair', {
      get_default_layout: () => ({
        source: 'generated',
        layout: null,
        config: { columns: [{ field_key: 'name', width: null }], sort: null, filters: [] },
      }),
      list_layouts: () => [],
      list_records: () => ({ items: [record()], total: 1, page: 1, page_size: 50 }),
    });
    const user = userEvent.setup();

    await screen.findByText('R202611-0001', { selector: 'td' }, { timeout: PAGE_LOAD_TIMEOUT });
    const headers = Array.from(document.querySelectorAll('.ant-card thead th')).map(
      (th) => th.textContent ?? '',
    );
    expect(headers[0]).toBe('單號');
    expect(headers).toContain('名稱');

    await user.click(
      document.querySelector<HTMLElement>('.ant-card thead th.ant-table-column-has-sorters')!,
    );
    await waitFor(() =>
      expect(calls(invoke, 'list_records').at(-1)?.query).toMatchObject({
        sort: { field_key: 'record_no', descending: false },
      }),
    );
  });
});
