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
  fakeListRecords,
} from '../../test/masterSplitFixtures';
import { renderWithProviders } from '../../test/render';

const PAGE_LOAD_TIMEOUT = 5000;

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
