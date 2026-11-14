import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type { Record as DataRecord } from '../../bindings';
import { ADMIN, VIEWER, startupCommands } from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import { REPAIR, entityDetail, fieldDef } from '../../test/schemaFixtures';

const PAGE_LOAD_TIMEOUT = 5000;

const NAME = fieldDef({
  field_key: 'name',
  label: '名稱',
  is_required: true,
  help_text: '客戶看得到的名稱',
  sort_order: 0,
});
const MEMO = fieldDef({ field_key: 'memo', label: '備註', data_type: 'long_text', sort_order: 1 });
const QTY = fieldDef({
  field_key: 'qty',
  label: '數量',
  data_type: 'integer',
  default_value: 1,
  validation: { min: '0', max: '100', max_length: null, pattern: null },
  sort_order: 2,
});
const RATE = fieldDef({
  field_key: 'rate',
  label: '折扣率',
  data_type: 'decimal',
  decimal_places: 4,
  sort_order: 3,
});
const PRICE = fieldDef({
  field_key: 'price',
  label: '金額',
  data_type: 'money',
  decimal_places: 2,
  sort_order: 4,
});
const DUE = fieldDef({ field_key: 'due', label: '到期日', data_type: 'date', sort_order: 5 });
const AT = fieldDef({
  field_key: 'at',
  label: '報修時間',
  data_type: 'datetime',
  default_value: '2026-11-13T01:05:00Z',
  sort_order: 6,
});
const DONE = fieldDef({ field_key: 'done', label: '完成', data_type: 'boolean', sort_order: 7 });
const STATUS = fieldDef({
  field_key: 'status',
  label: '狀態',
  data_type: 'select',
  options: [
    { value: 'open', label: '處理中', color: 'blue' },
    { value: 'closed', label: '已結案', color: 'green' },
  ],
  sort_order: 8,
});
const TAGS = fieldDef({
  field_key: 'tags',
  label: '標籤',
  data_type: 'multi_select',
  options: [
    { value: 'a', label: '急件', color: 'red' },
    { value: 'b', label: '保固', color: null },
  ],
  sort_order: 9,
});
const CODE = fieldDef({
  field_key: 'code',
  label: '編號',
  is_readonly: true,
  default_value: 'R-001',
  sort_order: 10,
});
const HIDDEN = fieldDef({ field_key: 'secret', label: '隱藏欄位', is_visible: false });
const INACTIVE = fieldDef({ field_key: 'old', label: '停用欄位', is_active: false });
const FIELDS = [TAGS, NAME, MEMO, QTY, RATE, PRICE, DUE, AT, DONE, STATUS, CODE, HIDDEN, INACTIVE];

const RECORD_ID = '0196f5a0-0000-7000-8000-000000000201';

function record(patch: Partial<DataRecord> = {}): DataRecord {
  return {
    id: RECORD_ID,
    title: '冷氣維修',
    data: {
      name: '冷氣維修',
      qty: 3,
      price: '1200.50',
      done: true,
      status: 'open',
      tags: ['a'],
      code: 'R-001',
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
    list_entities: () => [REPAIR],
    get_entity: () => entityDetail(FIELDS),
    get_record: () => record(),
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

/** 等表單出現（以「名稱」輸入框為準）。 */
async function waitForForm() {
  return screen.findByRole('textbox', { name: '名稱' }, { timeout: PAGE_LOAD_TIMEOUT });
}

/** 欄位的 Form.Item（標籤所在的區塊）。 */
function formItem(label: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`label[title="${label}"]`);
  if (!el) {
    throw new Error(`找不到欄位「${label}」`);
  }
  return el.closest<HTMLElement>('.ant-form-item')!;
}

function input(label: string): HTMLElement {
  return within(formItem(label)).getAllByLabelText(label)[0]!;
}

function calls(invoke: ReturnType<typeof mockCommands>, cmd: string) {
  return invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args);
}

describe('動態表單', () => {
  it('generates_input_for_each_type', async () => {
    renderAt('#/entities/repair/new');
    await waitForForm();

    // 依排序，只顯示有效且設定為顯示的欄位
    const labels = Array.from(document.querySelectorAll('.ant-form-item-label label')).map((l) =>
      l.getAttribute('title'),
    );
    expect(labels).toEqual([
      '名稱',
      '備註',
      '數量',
      '折扣率',
      '金額',
      '到期日',
      '報修時間',
      '完成',
      '狀態',
      '標籤',
      '編號',
    ]);

    expect(input('名稱').tagName).toBe('INPUT');
    expect(input('備註').tagName).toBe('TEXTAREA');
    expect(input('數量')).toHaveAttribute('inputmode', 'numeric');
    expect(input('折扣率')).toHaveAttribute('inputmode', 'decimal');
    expect(input('金額')).toHaveAttribute('inputmode', 'decimal');
    expect(input('到期日').closest('.ant-picker')).not.toBeNull();
    expect(input('報修時間').closest('.ant-picker')).not.toBeNull();
    expect(input('完成')).toHaveAttribute('type', 'checkbox');
    expect(input('狀態')).toHaveAttribute('role', 'combobox');
    expect(input('狀態').closest('.ant-select-multiple')).toBeNull();
    expect(input('標籤')).toHaveAttribute('role', 'combobox');
    expect(input('標籤').closest('.ant-select-multiple')).not.toBeNull();

    // 說明文字、必填標記、唯讀
    expect(within(formItem('名稱')).getByText('客戶看得到的名稱')).toBeInTheDocument();
    expect(formItem('名稱').querySelector('.ant-form-item-required')).not.toBeNull();
    expect(formItem('備註').querySelector('.ant-form-item-required')).toBeNull();
    expect(input('編號')).toBeDisabled();

    // 新增時套用預設值；日期時間以台北時間顯示
    expect(input('數量')).toHaveValue('1');
    expect(input('報修時間')).toHaveValue('2026-11-13 09:05');
    expect(input('編號')).toHaveValue('R-001');
  });

  it('decimal_values_are_sent_as_strings', async () => {
    const invoke = renderAt('#/entities/repair/new', {
      create_record: () => record({ version: 1 }),
    });
    const user = userEvent.setup();
    await waitForForm();

    await user.type(input('名稱'), '冷氣維修');
    await user.type(input('折扣率'), '0.1');
    await user.type(input('金額'), '0.2');
    await user.click(screen.getByRole('button', { name: /儲\s*存/ }));

    await waitFor(() => expect(calls(invoke, 'create_record')).toHaveLength(1));
    const data = (calls(invoke, 'create_record')[0]!.req as { data: Record<string, unknown> }).data;
    // 小數以字串送出，不會變成 0.1、0.2 的浮點數
    expect(data.rate).toBe('0.1');
    expect(data.price).toBe('0.2');
    expect(data).toEqual({
      name: '冷氣維修',
      memo: null,
      qty: 1,
      rate: '0.1',
      price: '0.2',
      due: null,
      at: '2026-11-13T09:05:00+08:00',
      done: false,
      status: null,
      tags: null,
    });
    // 新增後開啟該筆資料
    await waitFor(() => expect(window.location.hash).toBe(`#/entities/repair/${RECORD_ID}`));
  });

  it('client_validation_matches_server_rules', async () => {
    const invoke = renderAt('#/entities/repair/new', { create_record: () => record() });
    const user = userEvent.setup();
    await waitForForm();

    await user.clear(input('數量'));
    await user.type(input('數量'), '101');
    await user.type(input('金額'), '1.234');
    await user.click(screen.getByRole('button', { name: /儲\s*存/ }));

    expect(await within(formItem('名稱')).findByText('此欄位為必填')).toBeInTheDocument();
    expect(within(formItem('數量')).getByText('不可大於 100')).toBeInTheDocument();
    expect(within(formItem('金額')).getByText('小數位數不可超過 2 位')).toBeInTheDocument();
    expect(calls(invoke, 'create_record')).toHaveLength(0);
  });

  it('server_validation_errors_are_shown_on_fields', async () => {
    renderAt('#/entities/repair/new', {
      create_record: () =>
        commandError({
          kind: 'api',
          code: 'VALIDATION',
          message: '資料有誤',
          details: {
            fields: [
              { field_key: 'name', message: '「冷氣維修」已存在，不可重複' },
              { field_key: 'status', message: '「x」不是有效的選項' },
            ],
          },
        }),
    });
    const user = userEvent.setup();
    await waitForForm();

    await user.type(input('名稱'), '冷氣維修');
    await user.click(screen.getByRole('button', { name: /儲\s*存/ }));

    expect(
      await within(formItem('名稱')).findByText('「冷氣維修」已存在，不可重複'),
    ).toBeInTheDocument();
    expect(within(formItem('狀態')).getByText('「x」不是有效的選項')).toBeInTheDocument();
  });

  it('edit_loads_record_and_sends_changed_fields_with_version', async () => {
    const invoke = renderAt(`#/entities/repair/${RECORD_ID}`, {
      update_record: () => record({ version: 4, data: { ...record().data, price: '99.00' } }),
    });
    const user = userEvent.setup();
    await waitForForm();

    expect(input('名稱')).toHaveValue('冷氣維修');
    expect(input('金額')).toHaveValue('1200.50');
    expect(input('完成')).toBeChecked();
    expect(calls(invoke, 'get_record')[0]).toEqual({ key: 'repair', id: RECORD_ID });

    const save = screen.getByRole('button', { name: /儲\s*存/ });
    expect(save).toBeDisabled();

    await user.clear(input('金額'));
    await user.type(input('金額'), '99');
    await user.click(input('完成'));
    await user.click(save);

    await waitFor(() =>
      expect(calls(invoke, 'update_record')[0]).toEqual({
        key: 'repair',
        id: RECORD_ID,
        req: { version: 3, data: { price: '99', done: false } },
      }),
    );
    // 存檔後以伺服器回傳的資料重設表單
    await waitFor(() => expect(input('金額')).toHaveValue('99.00'));
    expect(screen.getByRole('button', { name: /儲\s*存/ })).toBeDisabled();
  });

  it('conflict_shows_message_and_reload', async () => {
    let version = 3;
    const invoke = renderAt(`#/entities/repair/${RECORD_ID}`, {
      get_record: () =>
        record({
          version,
          data: { ...record().data, name: version > 3 ? '別人改的' : '冷氣維修' },
        }),
      update_record: () =>
        commandError({ kind: 'api', code: 'CONFLICT', message: '版本不符', details: null }),
    });
    const user = userEvent.setup();
    await waitForForm();

    await user.clear(input('名稱'));
    await user.type(input('名稱'), '我改的');
    await user.click(screen.getByRole('button', { name: /儲\s*存/ }));

    expect(await screen.findByText('資料已被其他人修改')).toBeInTheDocument();

    version = 4;
    await user.click(screen.getByRole('button', { name: /重新載入/ }));
    await waitFor(() => expect(input('名稱')).toHaveValue('別人改的'));
    expect(calls(invoke, 'get_record')).toHaveLength(2);
    expect(screen.queryByText('資料已被其他人修改')).not.toBeInTheDocument();
  });

  it('warns_before_leaving_with_unsaved_changes', async () => {
    renderAt(`#/entities/repair/${RECORD_ID}`, {
      list_records: () => ({ items: [], total: 0, page: 1, page_size: 50 }),
      get_default_layout: () => ({
        source: 'generated',
        layout: null,
        config: { columns: [], sort: null, filters: [] },
      }),
      list_layouts: () => [],
    });
    const user = userEvent.setup();
    await waitForForm();

    await user.type(input('名稱'), '！');
    await user.click(screen.getByRole('button', { name: /返回列表/ }));

    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText('有未存檔的修改')).toBeInTheDocument();
    await user.click(within(dialog).getByRole('button', { name: /留\s*下/ }));
    expect(window.location.hash).toBe(`#/entities/repair/${RECORD_ID}`);
    expect(input('名稱')).toHaveValue('冷氣維修！');

    await user.click(screen.getByRole('button', { name: /返回列表/ }));
    await user.click(
      within(await screen.findByRole('dialog')).getByRole('button', { name: /離\s*開/ }),
    );
    await waitFor(() => expect(window.location.hash).toBe('#/entities/repair'));
  });

  it('viewer_can_only_view', async () => {
    renderAt(`#/entities/repair/${RECORD_ID}`, {}, VIEWER);
    await waitForForm();
    expect(input('名稱')).toBeDisabled();
    expect(input('金額')).toBeDisabled();
    expect(screen.queryByRole('button', { name: /儲\s*存/ })).not.toBeInTheDocument();
  });

  it('list_row_opens_record', async () => {
    renderAt('#/entities/repair', {
      list_records: () => ({ items: [record()], total: 1, page: 1, page_size: 50 }),
      get_default_layout: () => ({
        source: 'generated',
        layout: null,
        config: { columns: [{ field_key: 'name', width: null }], sort: null, filters: [] },
      }),
      list_layouts: () => [],
    });
    const user = userEvent.setup();
    await user.click(
      await screen.findByText('冷氣維修', { selector: 'td' }, { timeout: PAGE_LOAD_TIMEOUT }),
    );
    await waitFor(() => expect(window.location.hash).toBe(`#/entities/repair/${RECORD_ID}`));
    expect(await waitForForm()).toHaveValue('冷氣維修');
  });
});
