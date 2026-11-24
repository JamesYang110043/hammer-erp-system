import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type {
  CurrentUser,
  DefaultViewLayout_Serialize as DefaultViewLayout,
  Record as DataRecord,
  RecordListQuery_Deserialize as RecordListQuery,
} from '../../bindings';
import { ADMIN, VIEWER, startupCommands } from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import {
  CUSTOMER,
  CUSTOMER_CONTACT,
  CUSTOMER_CONTACT_FIELDS,
  CUSTOMER_FIELDS,
  CUSTOMER_FORM_LAYOUT,
  NAMED_CUSTOMERS,
  PRODUCT,
  PRODUCT_FIELDS,
  SUPPLIER,
  SUPPLIER_FIELDS,
  customerDetail,
  customerId,
  partyRecord,
  fakeListRecords,
} from '../../test/masterSplitFixtures';
import { renderWithProviders } from '../../test/render';

const PAGE_LOAD_TIMEOUT = 5000;
const NEW_ID = '0196f5a0-0000-7000-8000-00000000beef';

const FIELDS: Record<string, typeof CUSTOMER_FIELDS> = {
  customer: CUSTOMER_FIELDS,
  supplier: SUPPLIER_FIELDS,
  product: PRODUCT_FIELDS,
  customer_contact: CUSTOMER_CONTACT_FIELDS,
};
const ENTITIES = [CUSTOMER, CUSTOMER_CONTACT, SUPPLIER, PRODUCT];

const GENERATED: DefaultViewLayout = {
  source: 'generated',
  layout: null,
  config: { columns: [], sort: null, filters: [] },
};

function renderAt(
  hash: string,
  extra: Record<string, CommandHandler> = {},
  user: CurrentUser = ADMIN,
) {
  window.location.hash = hash;
  // 刪除後列表與單筆查詢都看不到那筆
  let customers: DataRecord[] = [...NAMED_CUSTOMERS];
  const invoke = mockCommands({
    ...startupCommands(user),
    list_entities: () => ENTITIES,
    get_entity: ({ key }) => ({
      entity: ENTITIES.find((e) => e.key === key),
      fields: FIELDS[key as string] ?? [],
    }),
    get_default_layout: () => GENERATED,
    list_layouts: () => [],
    list_records: ({ key, query }) =>
      fakeListRecords(key === 'customer' ? customers : [], query as RecordListQuery),
    get_record: ({ id }) =>
      customerDetail(customers, id as string) ??
      commandError({ kind: 'api', code: 'NOT_FOUND', message: '找不到資料', details: null }),
    get_form_layout: () => CUSTOMER_FORM_LAYOUT,
    delete_record: ({ id }) => {
      customers = customers.filter((c) => c.id !== id);
      return null;
    },
    update_record: ({ id, req }) => {
      const { data } = req as { data: Record<string, unknown> };
      customers = customers.map((c) =>
        c.id === id
          ? {
              ...c,
              data: { ...c.data, ...data },
              title: `${String({ ...c.data, ...data }.code)} ${String({ ...c.data, ...data }.name)}`,
              version: c.version + 1,
            }
          : c,
      );
      return customerDetail(customers, id as string);
    },
    create_record: ({ req }) => {
      const { data } = req as { data: Record<string, unknown> };
      const created = partyRecord(NEW_ID, data);
      customers = [...customers, created];
      return customerDetail(customers, NEW_ID);
    },
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

const listbox = () => screen.getByRole('listbox', { name: '客戶列表' });
const optionTexts = () =>
  within(listbox())
    .queryAllByRole('option')
    .map((o) => o.textContent);

async function waitForOptions() {
  await screen.findByRole('listbox', { name: '客戶列表' }, { timeout: PAGE_LOAD_TIMEOUT });
  await waitFor(() => expect(within(listbox()).getAllByRole('option').length).toBeGreaterThan(0));
}

function listQueries(invoke: ReturnType<typeof mockCommands>): RecordListQuery[] {
  return invoke.mock.calls
    .filter(([cmd]) => cmd === 'list_records')
    .map(([, args]) => args.query as RecordListQuery);
}

describe('分割檢視', () => {
  it('customer_shows_split_view_with_active_records_sorted_by_code', async () => {
    const invoke = renderAt('#/entities/customer');
    await waitForOptions();

    // 預設只顯示啟用中的資料（C003 停用）
    expect(optionTexts()).toEqual([
      'C001大華實業股份有限公司統編 12345675',
      'C002光明電子有限公司統編 87654321',
      'C004Global Trading Co.',
    ]);
    expect(screen.getByTestId('picker-total')).toHaveTextContent('共 3 筆');
    expect(screen.getByText('請從左側選擇客戶')).toBeInTheDocument();
    expect(listQueries(invoke)[0]).toEqual({
      page: 1,
      page_size: 50,
      sort: { field_key: 'code', descending: false },
      filters: [{ field_key: 'is_active', op: 'eq', value: 'true' }],
    });
  });

  it('supplier_also_uses_split_view', async () => {
    renderAt('#/entities/supplier');
    expect(
      await screen.findByRole('listbox', { name: '廠商列表' }, { timeout: PAGE_LOAD_TIMEOUT }),
    ).toBeInTheDocument();
    expect(screen.getByText('請從左側選擇廠商')).toBeInTheDocument();
  });

  it('other_entities_keep_the_table', async () => {
    renderAt('#/entities/product');
    await screen.findByRole('columnheader', { name: '料號' }, { timeout: PAGE_LOAD_TIMEOUT });
    expect(screen.queryByRole('listbox')).not.toBeInTheDocument();
  });

  it('all_filter_shows_inactive_records_in_gray', async () => {
    renderAt('#/entities/customer');
    await waitForOptions();
    const user = userEvent.setup();

    await user.click(screen.getByText('全部'));
    await waitFor(() => expect(optionTexts()).toHaveLength(4));
    const inactive = within(listbox()).getByRole('option', { name: /C003/ });
    expect(inactive).toHaveAttribute('data-inactive', 'true');
    expect(inactive).toHaveTextContent('停用');
    expect(screen.getByTestId('picker-total')).toHaveTextContent('共 4 筆');
  });

  it.each([
    ['code', 'c002', 'C002'],
    ['name', '光明', 'C002'],
    ['short_name', '大華', 'C001'],
    ['tax_id', '8765', 'C002'],
  ])('search_matches_%s', async (_field, q, code) => {
    const invoke = renderAt('#/entities/customer');
    await waitForOptions();
    const user = userEvent.setup();

    await user.type(screen.getByRole('textbox', { name: '搜尋客戶' }), q);
    await waitFor(() => expect(optionTexts()).toHaveLength(1));
    expect(optionTexts()[0]).toContain(code);
    // 每個搜尋欄位各查一次（伺服器的篩選條件是 AND）
    const searched = listQueries(invoke)
      .flatMap((query) => query.filters)
      .filter((f) => f.op === 'contains' && f.value === q)
      .map((f) => f.field_key);
    expect(searched).toEqual(['code', 'name', 'short_name', 'tax_id']);
  });

  it('search_without_matches_shows_empty_state', async () => {
    renderAt('#/entities/customer');
    await waitForOptions();
    const user = userEvent.setup();

    await user.type(screen.getByRole('textbox', { name: '搜尋客戶' }), '不存在');
    expect(await screen.findByText('找不到符合的資料')).toBeInTheDocument();
    expect(screen.getByTestId('picker-total')).toHaveTextContent('共 0 筆');
  });

  it('click_and_arrow_keys_change_selection_in_url', async () => {
    renderAt('#/entities/customer');
    await waitForOptions();
    const user = userEvent.setup();

    await user.click(within(listbox()).getByRole('option', { name: /C001/ }));
    await waitFor(() => expect(window.location.hash).toContain(`selected=${customerId(1)}`));
    expect(within(listbox()).getByRole('option', { name: /C001/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(await screen.findByTestId('preview-title')).toHaveTextContent(
      'C001 大華實業股份有限公司',
    );

    await user.keyboard('{ArrowDown}');
    await waitFor(() => expect(window.location.hash).toContain(`selected=${customerId(2)}`));
    await user.keyboard('{ArrowDown}');
    await waitFor(() => expect(window.location.hash).toContain(`selected=${customerId(4)}`));
    // 最後一筆再往下不變
    await user.keyboard('{ArrowDown}');
    await user.keyboard('{ArrowUp}');
    await waitFor(() => expect(window.location.hash).toContain(`selected=${customerId(2)}`));
  });

  it('selection_from_url_is_restored', async () => {
    renderAt(`#/entities/customer?selected=${customerId(2)}`);
    await waitForOptions();
    expect(within(listbox()).getByRole('option', { name: /C002/ })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    expect(listbox()).toHaveAttribute(
      'aria-activedescendant',
      `master-picker-option-${customerId(2)}`,
    );
  });
});

describe('右側唯讀預覽', () => {
  const preview = () => screen.getByTestId('record-preview');

  /** 預覽中依畫面順序的欄位標籤。 */
  const labels = () =>
    Array.from(preview().querySelectorAll('.preview-item > div:first-child')).map(
      (el) => el.textContent,
    );

  it('fields_follow_form_layout_and_contacts_are_listed', async () => {
    renderAt(`#/entities/customer?selected=${customerId(1)}`);
    expect(
      await screen.findByTestId('preview-title', {}, { timeout: PAGE_LOAD_TIMEOUT }),
    ).toHaveTextContent('C001 大華實業股份有限公司');

    // 依版面：名稱在編號前面；區塊標題
    expect(labels()).toEqual(['公司名稱', '客戶編號', '統一編號', '簡稱', '啟用', '電話']);
    expect(within(preview()).getByRole('region', { name: '基本資料' })).toBeInTheDocument();
    expect(within(preview()).getByRole('region', { name: '聯絡資訊' })).toBeInTheDocument();
    expect(within(preview()).getByText('12345675')).toBeInTheDocument();
    // 勾選欄位以唯讀勾選框顯示
    expect(within(preview()).getByRole('checkbox', { name: '啟用' })).toBeDisabled();

    // 聯絡人唯讀表格
    const contacts = within(preview()).getByRole('region', { name: '客戶聯絡人' });
    const rows = contacts.querySelectorAll('tbody tr.ant-table-row');
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent('王小明採購經理0912-345-678');
    expect(within(contacts).queryByRole('textbox')).not.toBeInTheDocument();
  });

  it('viewer_has_no_edit_or_delete', async () => {
    renderAt(`#/entities/customer?selected=${customerId(1)}`, {}, VIEWER);
    await screen.findByTestId('preview-title', {}, { timeout: PAGE_LOAD_TIMEOUT });
    expect(within(preview()).queryByRole('button', { name: /編\s*輯/ })).not.toBeInTheDocument();
    expect(within(preview()).queryByRole('button', { name: /刪\s*除/ })).not.toBeInTheDocument();
  });

  it('admin_sees_edit_and_delete', async () => {
    renderAt(`#/entities/customer?selected=${customerId(1)}`);
    await screen.findByTestId('preview-title', {}, { timeout: PAGE_LOAD_TIMEOUT });
    expect(within(preview()).getByRole('button', { name: /編\s*輯/ })).toBeInTheDocument();
    expect(within(preview()).getByRole('button', { name: /刪\s*除/ })).toBeInTheDocument();
  });

  it('delete_clears_selection_and_refreshes_list', async () => {
    const invoke = renderAt(`#/entities/customer?selected=${customerId(2)}`);
    await screen.findByTestId('preview-title', {}, { timeout: PAGE_LOAD_TIMEOUT });
    await waitForOptions();
    const user = userEvent.setup();

    await user.click(within(preview()).getByRole('button', { name: /刪\s*除/ }));
    await screen.findByText('確定要刪除這筆資料？');
    const popup = document.querySelector<HTMLElement>('.ant-popconfirm')!;
    await user.click(within(popup).getByRole('button', { name: /刪\s*除/ }));

    await waitFor(() => expect(window.location.hash).not.toContain('selected='));
    expect(await screen.findByText('請從左側選擇客戶')).toBeInTheDocument();
    await waitFor(() => expect(optionTexts().some((t) => t?.includes('C002'))).toBe(false));
    expect(invoke).toHaveBeenCalledWith('delete_record', {
      key: 'customer',
      id: customerId(2),
      version: 1,
    });
  });

  it('missing_record_shows_not_found', async () => {
    renderAt('#/entities/customer?selected=0196f5a0-0000-7000-8000-00000000dead');
    expect(
      await screen.findByText('找不到資料', {}, { timeout: PAGE_LOAD_TIMEOUT }),
    ).toBeInTheDocument();
  });
});

describe('在右側編輯與新增', () => {
  const panel = () => screen.getByTestId('record-form-panel');
  const nameInput = () => within(panel()).getByRole('textbox', { name: '公司名稱' });

  async function openEdit(user: ReturnType<typeof userEvent.setup>) {
    await screen.findByTestId('preview-title', {}, { timeout: PAGE_LOAD_TIMEOUT });
    await user.click(screen.getByRole('button', { name: /編\s*輯/ }));
    await screen.findByTestId('record-form-panel', {}, { timeout: PAGE_LOAD_TIMEOUT });
    // 左側列表仍然看得到
    expect(listbox()).toBeInTheDocument();
  }

  it('edit_saves_and_returns_to_preview_with_updated_list', async () => {
    const invoke = renderAt(`#/entities/customer?selected=${customerId(1)}`);
    const user = userEvent.setup();
    await waitForOptions();
    await openEdit(user);
    expect(window.location.hash).toContain('mode=edit');
    expect(within(panel()).getByText('編輯：C001 大華實業股份有限公司')).toBeInTheDocument();

    await user.clear(nameInput());
    await user.type(nameInput(), '大華科技');
    await user.click(within(panel()).getByRole('button', { name: /儲\s*存/ }));

    expect(
      await screen.findByTestId('preview-title', {}, { timeout: PAGE_LOAD_TIMEOUT }),
    ).toHaveTextContent('C001 大華科技');
    expect(window.location.hash).not.toContain('mode=');
    await waitFor(() => expect(optionTexts()[0]).toContain('C001大華科技'));
    expect(invoke).toHaveBeenCalledWith('update_record', {
      key: 'customer',
      id: customerId(1),
      req: { version: 1, data: { name: '大華科技' } },
    });
  });

  it('cancel_returns_to_preview', async () => {
    renderAt(`#/entities/customer?selected=${customerId(1)}`);
    const user = userEvent.setup();
    await openEdit(user);
    await user.click(within(panel()).getByRole('button', { name: /取\s*消/ }));
    expect(await screen.findByTestId('preview-title')).toHaveTextContent('C001');
  });

  it('new_opens_on_the_right_and_selects_created_record', async () => {
    const invoke = renderAt(`#/entities/customer?selected=${customerId(2)}`);
    const user = userEvent.setup();
    await waitForOptions();

    await user.click(screen.getByRole('button', { name: /新\s*增/ }));
    await screen.findByTestId('record-form-panel', {}, { timeout: PAGE_LOAD_TIMEOUT });
    expect(within(panel()).getByText('新增客戶')).toBeInTheDocument();
    await user.type(within(panel()).getByRole('textbox', { name: '客戶編號' }), 'C900');
    await user.type(nameInput(), '新客戶');
    await user.click(within(panel()).getByRole('button', { name: /儲\s*存/ }));

    expect(
      await screen.findByTestId('preview-title', {}, { timeout: PAGE_LOAD_TIMEOUT }),
    ).toHaveTextContent('C900 新客戶');
    await waitFor(() => expect(window.location.hash).toContain(`selected=${NEW_ID}`));
    expect(window.location.hash).not.toContain('mode=');
    await waitFor(() => expect(optionTexts().some((t) => t?.includes('C900新客戶'))).toBe(true));
    expect(invoke.mock.calls.filter(([cmd]) => cmd === 'create_record')).toHaveLength(1);
  });

  /**
   * 「有未存檔的修改」確認視窗按下 `button`。關閉的確認視窗在測試環境中會留在 DOM，
   * 所以以數量判斷這次是否有跳出新的視窗。
   */
  let answered = 0;
  const unsavedDialogs = () =>
    screen.queryAllByRole('dialog').filter((d) => d.textContent?.includes('有未存檔的修改'));
  async function answerUnsaved(user: ReturnType<typeof userEvent.setup>, button: RegExp) {
    await waitFor(() => expect(unsavedDialogs().length).toBe(answered + 1));
    answered += 1;
    const dialog = unsavedDialogs().at(-1)!;
    await user.click(within(dialog).getByRole('button', { name: button }));
  }

  it('unsaved_changes_confirm_before_switching_and_stay_keeps_them', async () => {
    answered = 0;
    renderAt(`#/entities/customer?selected=${customerId(1)}`);
    const user = userEvent.setup();
    await waitForOptions();
    await openEdit(user);
    await user.type(nameInput(), '（改）');

    // 點其他資料：跳出確認，選「留下」後修改還在
    await user.click(within(listbox()).getByRole('option', { name: /C002/ }));
    await answerUnsaved(user, /留\s*下/);
    expect(nameInput()).toHaveValue('大華實業股份有限公司（改）');
    expect(window.location.hash).toContain(`selected=${customerId(1)}`);

    // 搜尋、新增也要確認
    await user.type(screen.getByRole('textbox', { name: '搜尋客戶' }), 'x');
    await answerUnsaved(user, /留\s*下/);
    expect(screen.getByRole('textbox', { name: '搜尋客戶' })).toHaveValue('');
    await user.click(
      within(screen.getByTestId('page-header')).getByRole('button', { name: /新\s*增/ }),
    );
    await answerUnsaved(user, /留\s*下/);
    expect(window.location.hash).toContain('mode=edit');

    // 再點一次，選「離開」：切換到 C002 的預覽
    await user.click(within(listbox()).getByRole('option', { name: /C002/ }));
    await answerUnsaved(user, /離\s*開/);
    expect(await screen.findByTestId('preview-title')).toHaveTextContent('C002');
    expect(screen.queryByTestId('record-form-panel')).not.toBeInTheDocument();
  });

  it('conflict_shows_reload_prompt', async () => {
    renderAt(`#/entities/customer?selected=${customerId(1)}`, {
      update_record: () =>
        commandError({ kind: 'api', code: 'CONFLICT', message: '版本不符', details: null }),
    });
    const user = userEvent.setup();
    await openEdit(user);
    await user.type(nameInput(), '2');
    await user.click(within(panel()).getByRole('button', { name: /儲\s*存/ }));

    expect(await screen.findByText('資料已被其他人修改')).toBeInTheDocument();
    expect(within(panel()).getByRole('button', { name: /重新載入/ })).toBeInTheDocument();
  });

  it('old_record_url_redirects_to_split_view', async () => {
    renderAt(`#/entities/customer/${customerId(2)}`);
    expect(
      await screen.findByTestId('preview-title', {}, { timeout: PAGE_LOAD_TIMEOUT }),
    ).toHaveTextContent('C002');
    expect(window.location.hash).toBe(`#/entities/customer?selected=${customerId(2)}`);
  });

  it('old_new_url_opens_new_form_in_split_view', async () => {
    renderAt('#/entities/supplier/new');
    await screen.findByTestId('record-form-panel', {}, { timeout: PAGE_LOAD_TIMEOUT });
    expect(screen.getByText('新增廠商')).toBeInTheDocument();
    expect(screen.getByRole('listbox', { name: '廠商列表' })).toBeInTheDocument();
    expect(window.location.hash).toBe('#/entities/supplier?mode=new');
  });
});
