import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import dayjs from 'dayjs';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type { EntityDefinition } from '../../bindings';
import { KEY_MESSAGES } from '../../schema/keyRules';
import { PATTERN_MESSAGES } from '../../schema/numbering';
import { dragDownOneRow } from '../../test/dnd';
import { ADMIN, VIEWER, startupCommands } from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import { REPAIR, entityDetail } from '../../test/schemaFixtures';
import { TAIPEI_OFFSET_MINUTES } from '../../utils/time';

const PAGE_LOAD_TIMEOUT = 5000;

/** 已停用的主檔。 */
const EQUIPMENT: EntityDefinition = {
  ...REPAIR,
  id: '0196f5a0-0000-7000-8000-000000000101',
  key: 'equipment',
  label: '設備',
  icon: null,
  sort_order: 1,
  is_active: false,
  version: 3,
};

/** 有單號規則的單據。 */
const ORDER: EntityDefinition = {
  ...REPAIR,
  id: '0196f5a0-0000-7000-8000-000000000102',
  key: 'sales_order',
  label: '訂單',
  kind: 'document',
  numbering_pattern: 'SO{yyyyMM}-{0000}',
  version: 2,
};

/** 訂單的明細子表（不列在資料類別頁與選單）。 */
const ORDER_LINES: EntityDefinition = {
  ...REPAIR,
  id: '0196f5a0-0000-7000-8000-000000000103',
  key: 'order_lines',
  label: '訂單明細',
  kind: 'detail',
  parent_entity_id: ORDER.id,
};

const ALL = [REPAIR, EQUIPMENT, ORDER, ORDER_LINES];

function renderEntitiesPage(extra: Record<string, CommandHandler> = {}) {
  window.location.hash = '#/schema';
  const invoke = mockCommands({
    ...startupCommands(ADMIN),
    list_all_entities: () => ALL,
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

/** 資料類別所在的表格列。 */
async function rowOf(key: string) {
  const cell = await screen.findByText(
    key,
    { selector: 'td code' },
    { timeout: PAGE_LOAD_TIMEOUT },
  );
  return within(cell.closest('tr')!);
}

async function openCreateDialog() {
  const user = userEvent.setup();
  await rowOf('repair');
  await user.click(screen.getByRole('button', { name: /新增資料類別/ }));
  const dialog = within(await screen.findByRole('dialog'));
  return { user, dialog };
}

/** `userEvent.type` 的 `{` 是按鍵代碼的開頭，要寫成 `{{`。 */
function typed(text: string): string {
  return text.replaceAll('{', '{{');
}

function validationError(details: unknown, message = '驗證失敗') {
  return commandError({ kind: 'api', code: 'VALIDATION', message, details });
}

describe('資料類別頁', () => {
  it('lists_entities_grouped_by_kind_with_status', async () => {
    renderEntitiesPage();
    await rowOf('repair');

    const masters = within(screen.getByRole('region', { name: '主檔' }));
    expect(masters.getByText('repair')).toBeInTheDocument();
    const equipment = within(masters.getByText('equipment').closest('tr')!);
    expect(equipment.getByText('已停用')).toBeInTheDocument();
    expect(equipment.getByRole('button', { name: '啟用' })).toBeInTheDocument();

    const documents = within(screen.getByRole('region', { name: '單據' }));
    const order = within(documents.getByText('sales_order').closest('tr')!);
    expect(order.getByText('SO{yyyyMM}-{0000}')).toBeInTheDocument();
    expect(order.getByRole('button', { name: '停用' })).toBeInTheDocument();

    // 明細子表在所屬主表的頁面管理
    expect(screen.queryByText('order_lines')).not.toBeInTheDocument();
  });

  it('shows_numbering_example_while_typing', async () => {
    const created = { ...ORDER, key: 'repair_order', label: '維修工單', sort_order: 1 };
    const invoke = renderEntitiesPage({
      create_entity: () => ({ entity: created, fields: [] }),
      get_entity: () => ({ ...entityDetail([]), entity: created }),
    });
    const { user, dialog } = await openCreateDialog();

    await user.type(dialog.getByLabelText('代碼'), 'repair_order');
    await user.type(dialog.getByLabelText('名稱'), '維修工單');
    // antd 的 Radio.Button 隱藏了 input，點選項文字
    await user.click(dialog.getByText('單據'));
    expect(dialog.getByRole('radio', { name: '單據' })).toBeChecked();

    const pattern = dialog.getByLabelText('單號規則');
    await user.type(pattern, typed('RO{yyyyMM}'));
    expect(await dialog.findByText(PATTERN_MESSAGES.missingSequence)).toBeInTheDocument();
    expect(dialog.queryByText(/範例單號/)).not.toBeInTheDocument();

    await user.type(pattern, typed('-{0000}'));
    const month = dayjs().utcOffset(TAIPEI_OFFSET_MINUTES).format('YYYYMM');
    expect(await dialog.findByText(`RO${month}-0001`)).toBeInTheDocument();
    expect(dialog.getByText(/範例單號/)).toBeInTheDocument();
    await waitFor(() =>
      expect(dialog.queryByText(PATTERN_MESSAGES.missingSequence)).not.toBeInTheDocument(),
    );

    await user.clear(pattern);
    await user.type(pattern, typed('RO{yyyyMM}-{0000}-{000}'));
    expect(await dialog.findByText(PATTERN_MESSAGES.multipleSequences)).toBeInTheDocument();
    // 不合法時不送出
    await user.click(dialog.getByRole('button', { name: /建\s*立/ }));
    expect(invoke).not.toHaveBeenCalledWith('create_entity', expect.anything());

    await user.clear(pattern);
    await user.type(pattern, typed(' RO{yyyyMM}-{0000} '));
    await user.click(dialog.getByRole('button', { name: /建\s*立/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('create_entity', {
        req: {
          key: 'repair_order',
          label: '維修工單',
          icon: null,
          kind: 'document',
          parent_entity_id: null,
          numbering_pattern: 'RO{yyyyMM}-{0000}',
          title_template: null,
          // 排在單據的最後面
          sort_order: 1,
        },
      }),
    );
    expect(await screen.findByText('欄位定義：維修工單')).toBeInTheDocument();
    expect(window.location.hash).toBe('#/schema/repair_order');
  });

  it('validates_entity_key_naming_rule', async () => {
    const invoke = renderEntitiesPage();
    const { user, dialog } = await openCreateDialog();
    await user.type(dialog.getByLabelText('代碼'), 'Repair');
    expect(await dialog.findByText(KEY_MESSAGES.invalidStart)).toBeInTheDocument();
    await user.type(dialog.getByLabelText('名稱'), '維修');
    await user.click(dialog.getByRole('button', { name: /建\s*立/ }));
    expect(invoke).not.toHaveBeenCalledWith('create_entity', expect.anything());
  });

  it('shows_server_validation_under_field', async () => {
    renderEntitiesPage({
      create_entity: () =>
        validationError({ field: 'title_template' }, '標題範本引用了不存在的欄位「name」'),
    });
    const { user, dialog } = await openCreateDialog();
    await user.type(dialog.getByLabelText('代碼'), 'tickets');
    await user.type(dialog.getByLabelText('名稱'), '工單');
    await user.type(dialog.getByLabelText('標題範本'), typed('{name}'));
    await user.click(dialog.getByRole('button', { name: /建\s*立/ }));
    expect(await dialog.findByText('標題範本引用了不存在的欄位「name」')).toBeInTheDocument();
  });

  it('edits_entity', async () => {
    const invoke = renderEntitiesPage({
      update_entity: () => ({ entity: { ...ORDER, version: 3 }, fields: [] }),
    });
    const user = userEvent.setup();
    const row = await rowOf('sales_order');
    await user.click(row.getByRole('button', { name: '編輯' }));
    const dialog = within(await screen.findByRole('dialog'));

    expect(dialog.getByDisplayValue('sales_order')).toBeDisabled();
    expect(
      dialog.getByText('SO' + dayjs().utcOffset(TAIPEI_OFFSET_MINUTES).format('YYYYMM') + '-0001'),
    ).toBeInTheDocument();
    const label = dialog.getByLabelText('名稱');
    await user.clear(label);
    await user.type(label, '銷售訂單');
    await user.type(dialog.getByLabelText('標題範本'), typed('{record_no}'));
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('update_entity', {
        key: 'sales_order',
        req: {
          version: ORDER.version,
          key: 'sales_order',
          label: '銷售訂單',
          icon: ORDER.icon,
          numbering_pattern: ORDER.numbering_pattern,
          title_template: '{record_no}',
          sort_order: ORDER.sort_order,
          is_active: true,
        },
      }),
    );
  });

  it('deactivates_and_reactivates_entity', async () => {
    const invoke = renderEntitiesPage({
      update_entity: ({ key }) => ({ entity: { ...REPAIR, key: key as string }, fields: [] }),
    });
    const user = userEvent.setup();
    const repair = await rowOf('repair');
    await user.click(repair.getByRole('button', { name: '停用' }));
    const confirm = await screen.findByRole('tooltip');
    expect(within(confirm).getByText(/不會出現在選單/)).toBeInTheDocument();
    await user.click(within(confirm).getByRole('button', { name: /停\s*用/ }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('update_entity', {
        key: 'repair',
        req: expect.objectContaining({ version: REPAIR.version, is_active: false }),
      }),
    );
    expect(await screen.findByText('已停用「維修單」')).toBeInTheDocument();

    const equipment = await rowOf('equipment');
    await user.click(equipment.getByRole('button', { name: '啟用' }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('update_entity', {
        key: 'equipment',
        req: expect.objectContaining({ version: EQUIPMENT.version, is_active: true }),
      }),
    );
  });

  it('delete_rejected_lists_referencing_fields', async () => {
    const invoke = renderEntitiesPage({
      delete_entity: () =>
        validationError(
          {
            field: 'relation_entity_id',
            fields: [
              {
                entity_key: 'sales_order',
                entity_label: '訂單',
                field_key: 'repair',
                field_label: '維修單號',
              },
              {
                entity_key: 'order_lines',
                entity_label: '訂單明細',
                field_key: 'source_repair',
                field_label: '來源維修單',
              },
            ],
          },
          '還有關聯欄位指向這個資料類別',
        ),
    });
    const user = userEvent.setup();
    const row = await rowOf('repair');
    await user.click(row.getByRole('button', { name: '刪除' }));
    const confirm = await screen.findByRole('tooltip');
    expect(within(confirm).getByText(/明細子表會一起刪除/)).toBeInTheDocument();
    await user.click(within(confirm).getByRole('button', { name: /刪\s*除/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('delete_entity', {
        key: 'repair',
        version: REPAIR.version,
      }),
    );
    const dialog = within(await screen.findByRole('dialog'));
    expect(dialog.getAllByText('無法刪除「維修單」').length).toBeGreaterThan(0);
    expect(dialog.getByText(/2 個關聯欄位還指向這個資料類別/)).toBeInTheDocument();
    expect(dialog.getByText('維修單號')).toBeInTheDocument();
    expect(dialog.getByText('source_repair')).toBeInTheDocument();
    expect(dialog.getByText('訂單明細')).toBeInTheDocument();
  });

  it('deletes_entity', async () => {
    let entities = ALL;
    const invoke = renderEntitiesPage({
      list_all_entities: () => entities,
      delete_entity: () => {
        entities = ALL.filter((e) => e.key !== 'equipment');
        return null;
      },
    });
    const user = userEvent.setup();
    const row = await rowOf('equipment');
    await user.click(row.getByRole('button', { name: '刪除' }));
    await user.click(
      within(await screen.findByRole('tooltip')).getByRole('button', { name: /刪\s*除/ }),
    );
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('delete_entity', {
        key: 'equipment',
        version: EQUIPMENT.version,
      }),
    );
    expect(await screen.findByText('已刪除「設備」')).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.queryByText('equipment', { selector: 'td code' })).not.toBeInTheDocument(),
    );
  });

  it('system_entity_cannot_be_deactivated_or_deleted', async () => {
    const system = { ...REPAIR, id: 'system-customer', key: 'customer', is_system: true };
    renderEntitiesPage({ list_all_entities: () => [system] });
    const row = await rowOf('customer');
    expect(row.getByText('系統')).toBeInTheDocument();
    expect(row.getByRole('button', { name: '編輯' })).toBeInTheDocument();
    expect(row.queryByRole('button', { name: '停用' })).not.toBeInTheDocument();
    expect(row.queryByRole('button', { name: '刪除' })).not.toBeInTheDocument();
  });

  it('system_entity_edit_hides_numbering_and_allows_relation_title', async () => {
    const system = {
      ...REPAIR,
      id: 'system-product',
      key: 'product',
      label: '商品',
      is_system: true,
      storage: 'table' as const,
      title_template: '{code} {name}',
    };
    const invoke = renderEntitiesPage({
      list_all_entities: () => [system],
      update_entity: () =>
        validationError(
          { field: 'title_template' },
          '標題範本引用的「unit_id.nam」不是關聯資料類別的欄位',
        ),
    });
    const user = userEvent.setup();
    const row = await rowOf('product');
    await user.click(row.getByRole('button', { name: '編輯' }));
    const dialog = within(await screen.findByRole('dialog'));

    // 系統資料類別不可設定單號規則
    expect(dialog.queryByLabelText('單號規則')).not.toBeInTheDocument();
    expect(dialog.getByText(/關聯欄位\.目標欄位/)).toBeInTheDocument();
    const title = dialog.getByLabelText('標題範本');
    await user.clear(title);
    await user.type(title, typed('{code} {unit_id.nam}'));
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));

    // 伺服器的錯誤顯示在欄位下方
    expect(
      await dialog.findByText('標題範本引用的「unit_id.nam」不是關聯資料類別的欄位'),
    ).toBeInTheDocument();
    expect(invoke).toHaveBeenCalledWith('update_entity', {
      key: 'product',
      req: expect.objectContaining({
        numbering_pattern: null,
        title_template: '{code} {unit_id.nam}',
      }),
    });
  });

  it('reorders_entities_within_group_by_keyboard_drag', async () => {
    const invoke = renderEntitiesPage({ reorder_entities: () => [] });
    const user = userEvent.setup();
    await rowOf('repair');

    await dragDownOneRow(user, '維修單');

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('reorder_entities', {
        req: {
          entities: [
            { key: 'equipment', version: EQUIPMENT.version, sort_order: 0 },
            { key: 'repair', version: REPAIR.version, sort_order: 1 },
          ],
        },
      }),
    );
  });

  it('requires_schema_manage', async () => {
    window.location.hash = '#/schema/repair';
    mockCommands(startupCommands(VIEWER));
    renderWithProviders(<App />);
    expect(
      await screen.findByText('沒有權限', undefined, { timeout: PAGE_LOAD_TIMEOUT }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: /資料類別/ })).not.toBeInTheDocument();
  });
});

describe('左側選單', () => {
  it('groups_entities_by_kind_without_details', async () => {
    window.location.hash = '#/connection';
    const SECOND_ORDER = {
      ...ORDER,
      id: 'entity-quote',
      key: 'quote',
      label: '報價單',
      sort_order: -1,
    };
    mockCommands({
      ...startupCommands(VIEWER),
      // 選單只會收到啟用中的資料類別
      list_entities: () => [REPAIR, ORDER, SECOND_ORDER, ORDER_LINES],
      health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    });
    renderWithProviders(<App />);

    const repair = await screen.findByRole(
      'menuitem',
      { name: /維修單/ },
      { timeout: PAGE_LOAD_TIMEOUT },
    );
    expect(
      screen.getByText('主檔', { selector: '.ant-menu-item-group-title' }),
    ).toBeInTheDocument();
    expect(
      screen.getByText('單據', { selector: '.ant-menu-item-group-title' }),
    ).toBeInTheDocument();
    expect(screen.queryByRole('menuitem', { name: /訂單明細/ })).not.toBeInTheDocument();

    // 單據組內依排序：報價單（-1）在訂單（0）前面
    const items = screen.getAllByRole('menuitem').map((el) => el.textContent);
    expect(items.indexOf('報價單')).toBeLessThan(items.indexOf('訂單'));
    expect(items.indexOf('維修單')).toBeLessThan(items.indexOf('報價單'));
    expect(repair).toBeInTheDocument();
  });
});

describe('重新計算公式', () => {
  it('recalculates_after_confirm_and_shows_result', async () => {
    const invoke = renderEntitiesPage({
      recalculate_entity: () => ({ processed: 12, changed: 3, with_errors: 1 }),
    });
    const user = userEvent.setup();
    const row = await rowOf('repair');
    await user.click(row.getByRole('button', { name: '重新計算' }));

    // 先確認，確認前不呼叫
    const confirm = await screen.findByRole('tooltip');
    expect(within(confirm).getByText('確定要重新計算「維修單」？')).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith('recalculate_entity', expect.anything());
    await user.click(within(confirm).getByRole('button', { name: '重新計算' }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('recalculate_entity', { key: 'repair' }),
    );
    expect(await screen.findByText('處理 12 筆，其中 3 筆有變動。')).toBeInTheDocument();
    expect(screen.getByText(/有 1 筆資料的公式計算錯誤/)).toBeInTheDocument();
  });

  it('only_active_custom_master_or_document_can_recalculate', async () => {
    renderEntitiesPage();
    expect((await rowOf('repair')).getByRole('button', { name: '重新計算' })).toBeInTheDocument();
    expect(
      (await rowOf('sales_order')).getByRole('button', { name: '重新計算' }),
    ).toBeInTheDocument();
    // 已停用
    expect(
      (await rowOf('equipment')).queryByRole('button', { name: '重新計算' }),
    ).not.toBeInTheDocument();
  });
});
