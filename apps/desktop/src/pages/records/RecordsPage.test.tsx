import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type {
  DefaultViewLayout_Serialize as DefaultViewLayout,
  Page,
  Record as DataRecord,
  ViewLayout_Serialize as ViewLayout,
} from '../../bindings';
import { ADMIN, VIEWER, startupCommands } from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import { REPAIR, entityDetail, fieldDef } from '../../test/schemaFixtures';

const PAGE_LOAD_TIMEOUT = 5000;

const NAME = fieldDef({ field_key: 'name', label: '名稱', sort_order: 0 });
const PRICE = fieldDef({
  field_key: 'price',
  label: '金額',
  data_type: 'money',
  decimal_places: 2,
  sort_order: 1,
});
const DUE = fieldDef({ field_key: 'due', label: '到期日', data_type: 'date', sort_order: 2 });
const DONE = fieldDef({ field_key: 'done', label: '完成', data_type: 'boolean', sort_order: 3 });
const STATUS = fieldDef({
  field_key: 'status',
  label: '狀態',
  data_type: 'select',
  options: [
    { value: 'open', label: '處理中', color: 'blue' },
    { value: 'closed', label: '已結案', color: 'green' },
  ],
  sort_order: 4,
});
const TAGS = fieldDef({
  field_key: 'tags',
  label: '標籤',
  data_type: 'multi_select',
  options: [
    { value: 'a', label: '急件', color: 'red' },
    { value: 'b', label: '保固', color: null },
  ],
  sort_order: 5,
});
const MEMO = fieldDef({ field_key: 'memo', label: '備註', is_visible: false, sort_order: 6 });
const FIELDS = [NAME, PRICE, DUE, DONE, STATUS, TAGS, MEMO];

const RECORD: DataRecord = {
  id: '0196f5a0-0000-7000-8000-000000000201',
  title: '冷氣維修',
  data: {
    name: '冷氣維修',
    price: '1234567.5',
    due: '2026-11-20',
    done: true,
    status: 'open',
    tags: ['a', 'b'],
    memo: '隱藏的備註',
  },
  created_at: '2026-11-13T01:00:00Z',
  created_by: null,
  updated_at: '2026-11-13T01:00:00Z',
  updated_by: null,
  version: 3,
};

function page(items: DataRecord[], total = items.length, p = 1, size = 50): Page<DataRecord> {
  return { items, total, page: p, page_size: size };
}

const GENERATED: DefaultViewLayout = {
  source: 'generated',
  layout: null,
  config: {
    columns: ['name', 'price', 'due', 'done', 'status', 'tags'].map((k) => ({
      field_key: k,
      width: null,
    })),
    sort: null,
    filters: [],
  },
};

function layout(patch: Partial<ViewLayout> = {}): ViewLayout {
  return {
    id: '0196f5a0-0000-7000-8000-000000000301',
    user_id: ADMIN.id,
    name: '只看名稱與備註',
    config: {
      columns: [
        { field_key: 'name', width: 240 },
        { field_key: 'memo', width: null },
      ],
      sort: { field_key: 'name', descending: false },
      filters: [{ field_key: 'status', op: 'eq', value: 'open' }],
    },
    is_default: false,
    created_at: '2026-11-13T01:00:00Z',
    updated_at: '2026-11-13T01:00:00Z',
    version: 1,
    ...patch,
  };
}

const EMPTY_QUERY = { page: 1, page_size: 50, sort: null, filters: [] };

function renderRecordsPage(
  extra: Record<string, CommandHandler> = {},
  user = ADMIN,
): ReturnType<typeof mockCommands> {
  window.location.hash = '#/entities/repair';
  const invoke = mockCommands({
    ...startupCommands(user),
    list_entities: () => [REPAIR],
    get_entity: () => entityDetail(FIELDS),
    get_default_layout: () => GENERATED,
    list_layouts: () => [],
    list_records: () => page([RECORD]),
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

/** 等資料出現，回傳第一列。 */
async function firstRow() {
  const cell = await screen.findByText(
    '冷氣維修',
    { selector: 'td' },
    { timeout: PAGE_LOAD_TIMEOUT },
  );
  return within(cell.closest('tr')!);
}

function headers(): string[] {
  // 只看列表本身（不含對話框中的表格）
  return Array.from(document.querySelectorAll('.ant-card thead th'))
    .map((th) => th.textContent ?? '')
    .filter((t) => t !== '');
}

function recordQueries(invoke: ReturnType<typeof mockCommands>) {
  return invoke.mock.calls.filter(([cmd]) => cmd === 'list_records').map(([, args]) => args);
}

function lastQuery(invoke: ReturnType<typeof mockCommands>) {
  return recordQueries(invoke).at(-1)?.query;
}

/** antd Select 的輸入框（`aria-label` 也會出現在下拉清單上）。 */
function selects(label: string): HTMLElement[] {
  return screen.getAllByLabelText(label).filter((el) => el.getAttribute('role') === 'combobox');
}

/** Popconfirm 的確認按鈕。 */
function confirmButton(): HTMLElement {
  const popup = document.querySelector<HTMLElement>('.ant-popconfirm')!;
  return within(popup).getByRole('button', { name: /刪\s*除/ });
}

/** 選擇 antd Select 的選項（同名的有多個時選最後一個）。 */
async function choose(user: ReturnType<typeof userEvent.setup>, label: string, option: string) {
  await user.click(selects(label).at(-1)!);
  const options = await screen.findAllByTitle(option);
  await user.click(options.at(-1)!);
}

describe('動態列表', () => {
  it('generates_columns_and_type_display_from_field_definitions', async () => {
    const invoke = renderRecordsPage();
    const row = await firstRow();

    // 依欄位定義：有效且顯示的欄位，依排序；隱藏的「備註」不顯示
    expect(headers()).toEqual(['名稱', '金額', '到期日', '完成', '狀態', '標籤', '操作']);
    expect(screen.queryByText('隱藏的備註')).not.toBeInTheDocument();

    // 金額千分位並補足小數位數、日期、勾選、選項顏色標籤
    expect(row.getByText('1,234,567.50')).toBeInTheDocument();
    expect(row.getByText('2026-11-20')).toBeInTheDocument();
    expect(row.getByRole('checkbox', { name: '完成' })).toBeChecked();
    expect(row.getByText('處理中')).toHaveClass('ant-tag-blue');
    expect(row.getByText('急件')).toHaveClass('ant-tag-red');
    expect(row.getByText('保固')).toBeInTheDocument();

    expect(recordQueries(invoke)[0]).toEqual({ key: 'repair', query: EMPTY_QUERY });
  });

  it('sidebar_lists_active_custom_entities', async () => {
    renderRecordsPage({
      list_entities: () => [
        REPAIR,
        { ...REPAIR, id: 'e2', key: 'visit', label: '拜訪紀錄', sort_order: 1 },
        { ...REPAIR, id: 'e3', key: 'customer', label: '客戶主檔', storage: 'table' },
      ],
    });
    const user = userEvent.setup();
    await firstRow();

    const menu = screen.getByRole('menu');
    expect(within(menu).getByRole('menuitem', { name: /維修單/ })).toHaveClass(
      'ant-menu-item-selected',
    );
    expect(within(menu).queryByRole('menuitem', { name: /客戶主檔/ })).not.toBeInTheDocument();

    await user.click(within(menu).getByRole('menuitem', { name: /拜訪紀錄/ }));
    expect(window.location.hash).toBe('#/entities/visit');
  });

  it('sort_sends_sort_param', async () => {
    const invoke = renderRecordsPage();
    const user = userEvent.setup();
    await firstRow();

    const title = screen.getByText('金額', { selector: 'thead th .ant-table-column-title' });
    await user.click(title);
    await waitFor(() =>
      expect(lastQuery(invoke)).toEqual({
        ...EMPTY_QUERY,
        sort: { field_key: 'price', descending: false },
      }),
    );
    await user.click(title);
    await waitFor(() =>
      expect(lastQuery(invoke)).toEqual({
        ...EMPTY_QUERY,
        sort: { field_key: 'price', descending: true },
      }),
    );
    // 第三次取消排序：依建立順序
    await user.click(title);
    await waitFor(() => expect(lastQuery(invoke)).toEqual(EMPTY_QUERY));
  });

  it('filters_send_typed_params', async () => {
    const invoke = renderRecordsPage();
    const user = userEvent.setup();
    await firstRow();

    // 金額 ≥ 100（數值以字串送出）
    await user.click(screen.getByRole('button', { name: /新增條件/ }));
    await choose(user, '篩選欄位', '金額');
    await choose(user, '篩選條件', '大於等於');
    await user.type(screen.getByLabelText('金額的篩選值'), '100.50');

    // 標籤包含任一：急件、保固
    await user.click(screen.getByRole('button', { name: /新增條件/ }));
    await user.click(selects('篩選欄位')[1]!);
    await user.click((await screen.findAllByTitle('標籤')).at(-1)!);
    await choose(user, '標籤的篩選值', '急件');
    await user.click((await screen.findAllByTitle('保固')).at(-1)!);

    // 狀態沒有值
    await user.click(screen.getByRole('button', { name: /新增條件/ }));
    await user.click(selects('篩選欄位')[2]!);
    await user.click((await screen.findAllByTitle('狀態')).at(-1)!);
    await user.click(selects('篩選條件')[2]!);
    await user.click((await screen.findAllByTitle('沒有值')).at(-1)!);

    await user.click(screen.getByRole('button', { name: /套用篩選/ }));

    await waitFor(() =>
      expect(lastQuery(invoke)).toEqual({
        ...EMPTY_QUERY,
        filters: [
          { field_key: 'price', op: 'gte', value: '100.50' },
          { field_key: 'tags', op: 'any', value: 'a' },
          { field_key: 'tags', op: 'any', value: 'b' },
          { field_key: 'status', op: 'empty', value: 'true' },
        ],
      }),
    );
  });

  it('pagination_sends_page_params', async () => {
    const invoke = renderRecordsPage({ list_records: () => page([RECORD], 120) });
    const user = userEvent.setup();
    await firstRow();

    await user.click(screen.getByTitle('2'));
    await waitFor(() => expect(lastQuery(invoke)).toEqual({ ...EMPTY_QUERY, page: 2 }));
  });

  it('applies_default_layout_on_open', async () => {
    const saved = layout({ is_default: true });
    const invoke = renderRecordsPage({
      get_default_layout: () => ({ source: 'personal', layout: saved, config: saved.config }),
      list_layouts: () => [saved],
    });
    await firstRow();

    expect(headers()).toEqual(['名稱', '備註', '操作']);
    expect(screen.getByText('隱藏的備註')).toBeInTheDocument();
    expect(recordQueries(invoke)[0]?.query).toEqual({
      ...EMPTY_QUERY,
      sort: saved.config.sort,
      filters: saved.config.filters,
    });
  });

  it('switching_layout_changes_columns', async () => {
    const other = layout();
    const invoke = renderRecordsPage({ list_layouts: () => [other] });
    const user = userEvent.setup();
    await firstRow();
    expect(headers()).toContain('金額');

    await choose(user, '版面', '只看名稱與備註');

    await waitFor(() => expect(headers()).toEqual(['名稱', '備註', '操作']));
    await waitFor(() =>
      expect(lastQuery(invoke)).toEqual({
        ...EMPTY_QUERY,
        sort: other.config.sort,
        filters: other.config.filters,
      }),
    );

    // 切回依欄位定義
    await choose(user, '版面', '依欄位定義');
    await waitFor(() => expect(headers()).toContain('金額'));
  });

  it('column_settings_then_save_as_layout', async () => {
    const created = layout({ id: 'new-layout', name: '我的版面', is_default: true });
    const invoke = renderRecordsPage({ create_layout: () => created });
    const user = userEvent.setup();
    await firstRow();

    // 隱藏金額、顯示備註並設定欄寬
    await user.click(screen.getByRole('button', { name: /欄位設定/ }));
    const dialog = within(await screen.findByRole('dialog'));
    await user.click(dialog.getByLabelText('顯示金額'));
    await user.click(dialog.getByLabelText('顯示備註'));
    await user.type(dialog.getByLabelText('備註的欄寬'), '180');
    await user.click(dialog.getByRole('button', { name: /套\s*用/ }));

    await waitFor(() =>
      expect(headers()).toEqual(['名稱', '到期日', '完成', '狀態', '標籤', '備註', '操作']),
    );
    expect(screen.getByText('未儲存的變更')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: /另存新版面/ }));
    const nameInput = await screen.findByLabelText('版面名稱');
    const saveDialog = within(nameInput.closest<HTMLElement>('.ant-modal')!);
    await user.type(nameInput, '我的版面');
    await user.click(saveDialog.getByRole('button', { name: /儲\s*存/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('create_layout', {
        key: 'repair',
        req: {
          name: '我的版面',
          is_global: false,
          is_default: true,
          config: {
            columns: [
              { field_key: 'name', width: null },
              { field_key: 'due', width: null },
              { field_key: 'done', width: null },
              { field_key: 'status', width: null },
              { field_key: 'tags', width: null },
              { field_key: 'memo', width: 180 },
            ],
            sort: null,
            filters: [],
          },
        },
      }),
    );
  });

  it('save_overwrites_own_layout_with_version', async () => {
    const mine = layout({ is_default: true });
    const invoke = renderRecordsPage({
      get_default_layout: () => ({ source: 'personal', layout: mine, config: mine.config }),
      list_layouts: () => [mine],
      update_layout: () => ({ ...mine, version: 2 }),
    });
    const user = userEvent.setup();
    await firstRow();

    await user.click(screen.getByText('名稱', { selector: 'thead th .ant-table-column-title' }));
    await user.click(screen.getByRole('button', { name: /儲存版面/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('update_layout', {
        key: 'repair',
        id: mine.id,
        req: {
          version: 1,
          config: { ...mine.config, sort: { field_key: 'name', descending: true } },
        },
      }),
    );
  });

  it('global_layout_is_read_only_without_schema_manage', async () => {
    const global = layout({ user_id: null, name: '公司版面', is_default: true });
    renderRecordsPage(
      {
        get_default_layout: () => ({ source: 'global', layout: global, config: global.config }),
        list_layouts: () => [global],
      },
      VIEWER,
    );
    await firstRow();
    expect(screen.queryByRole('button', { name: /儲存版面/ })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /刪除版面/ })).not.toBeInTheDocument();
    // 唯讀者仍可另存個人版面，但不能選全域
    expect(screen.getByRole('button', { name: /另存新版面/ })).toBeInTheDocument();
  });

  it('viewer_cannot_see_create_and_delete_buttons', async () => {
    renderRecordsPage({}, VIEWER);
    const row = await firstRow();
    expect(screen.queryByRole('button', { name: /新\s*增$/ })).not.toBeInTheDocument();
    expect(row.queryByRole('button', { name: '刪除' })).not.toBeInTheDocument();
    expect(headers()).not.toContain('操作');
  });

  it('editor_sees_create_and_delete_buttons', async () => {
    renderRecordsPage();
    const row = await firstRow();
    expect(screen.getByRole('button', { name: /新\s*增$/ })).toBeInTheDocument();
    expect(row.getByRole('button', { name: '刪除' })).toBeInTheDocument();
  });

  it('delete_asks_for_confirmation_and_sends_version', async () => {
    const invoke = renderRecordsPage({ delete_record: () => null });
    const user = userEvent.setup();
    const row = await firstRow();

    await user.click(row.getByRole('button', { name: '刪除' }));
    expect(invoke).not.toHaveBeenCalledWith('delete_record', expect.anything());
    expect(await screen.findByText('確定要刪除這筆資料？')).toBeInTheDocument();
    await user.click(confirmButton());
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('delete_record', {
        key: 'repair',
        id: RECORD.id,
        version: 3,
      }),
    );
  });

  it('delete_conflict_shows_message', async () => {
    renderRecordsPage({
      delete_record: () =>
        commandError({ kind: 'api', code: 'CONFLICT', message: '版本不符', details: null }),
    });
    const user = userEvent.setup();
    const row = await firstRow();
    await user.click(row.getByRole('button', { name: '刪除' }));
    await screen.findByText('確定要刪除這筆資料？');
    await user.click(confirmButton());
    expect(await screen.findByText('資料已被其他人修改，請重新整理')).toBeInTheDocument();
  });

  it('invalid_filter_error_is_shown', async () => {
    renderRecordsPage({
      list_records: () =>
        commandError({
          kind: 'api',
          code: 'VALIDATION',
          message: '沒有可查詢的欄位「old」',
          details: { field: 'filter.old.eq' },
        }),
    });
    expect(
      await screen.findByText('沒有可查詢的欄位「old」', undefined, { timeout: PAGE_LOAD_TIMEOUT }),
    ).toBeInTheDocument();
  });
});
