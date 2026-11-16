import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import { CONFLICT_MESSAGE } from '../../api/errors';
import type { FieldDefinition } from '../../bindings';
import { KEY_MESSAGES } from '../../schema/keyRules';
import { ADMIN, startupCommands } from '../../test/fixtures';
import { dragDownOneRow } from '../../test/dnd';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import {
  CUSTOM_FIELD,
  REPAIR,
  SYSTEM_FIELD,
  entityDetail,
  fieldDef,
} from '../../test/schemaFixtures';

/** 第一次載入（啟動流程 → 路由 → 欄位列表）的等待上限，見 UsersPage.test.tsx。 */
const PAGE_LOAD_TIMEOUT = 5000;

/** 以管理者登入，開啟 `repair` 的欄位定義頁。 */
function renderFieldsPage(extra: Record<string, CommandHandler> = {}) {
  window.location.hash = '#/schema/repair';
  const invoke = mockCommands({
    ...startupCommands(ADMIN),
    get_entity: () => entityDetail(),
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

/** 欄位所在的表格列。 */
async function rowOf(fieldKey: string) {
  const cell = await screen.findByText(
    fieldKey,
    { selector: 'td code' },
    { timeout: PAGE_LOAD_TIMEOUT },
  );
  return within(cell.closest('tr')!);
}

async function openNewFieldDialog() {
  const user = userEvent.setup();
  await rowOf(CUSTOM_FIELD.field_key);
  await user.click(screen.getByRole('button', { name: /新增欄位/ }));
  const dialog = within(await screen.findByRole('dialog'));
  return { user, dialog };
}

async function chooseOption(
  user: ReturnType<typeof userEvent.setup>,
  label: string,
  title: string,
) {
  await user.click(screen.getByLabelText(label));
  const options = await screen.findAllByTitle(title);
  await user.click(options[options.length - 1]!);
}

function validationError(details: unknown, message = '驗證失敗') {
  return commandError({ kind: 'api', code: 'VALIDATION', message, details });
}

describe('欄位定義頁', () => {
  it('lists_fields_with_flags', async () => {
    renderFieldsPage({
      get_entity: () =>
        entityDetail([
          SYSTEM_FIELD,
          CUSTOM_FIELD,
          fieldDef({ field_key: 'note', label: '備註', is_active: false, sort_order: 2 }),
        ]),
    });

    const system = await rowOf('record_no');
    expect(system.getByText('單號')).toBeInTheDocument();
    expect(system.getByText('系統')).toBeInTheDocument();
    expect(system.getByText('必填')).toBeInTheDocument();
    expect(system.getByText('不可重複')).toBeInTheDocument();
    // 系統欄位不能停用、不能變更型態
    expect(system.queryByRole('button', { name: '停用' })).not.toBeInTheDocument();
    expect(system.queryByRole('button', { name: '變更型態' })).not.toBeInTheDocument();

    const custom = await rowOf('qty');
    expect(custom.getByText('整數')).toBeInTheDocument();
    expect(custom.getByRole('button', { name: '停用' })).toBeInTheDocument();
    expect(custom.getByRole('button', { name: '變更型態' })).toBeInTheDocument();

    const inactive = await rowOf('note');
    expect(inactive.getByText('已停用')).toBeInTheDocument();
    expect(inactive.getByRole('button', { name: '啟用' })).toBeInTheDocument();
    expect(screen.getByText(`欄位定義：${REPAIR.label}`)).toBeInTheDocument();
  });

  it('shows_settings_for_each_data_type', async () => {
    renderFieldsPage();
    const { user, dialog } = await openNewFieldDialog();

    // 預設單行文字：字元數上限、格式規則
    expect(dialog.getByLabelText('字元數上限')).toBeInTheDocument();
    expect(dialog.getByLabelText('格式規則')).toBeInTheDocument();
    expect(dialog.queryByLabelText('小數位數')).not.toBeInTheDocument();

    await chooseOption(user, '資料型態', '小數');
    expect(dialog.getByLabelText('小數位數')).toBeInTheDocument();
    expect(dialog.getByLabelText('最小值')).toBeInTheDocument();
    expect(dialog.getByLabelText('最大值')).toBeInTheDocument();
    expect(dialog.queryByLabelText('字元數上限')).not.toBeInTheDocument();
    expect(dialog.queryByLabelText('格式規則')).not.toBeInTheDocument();

    await chooseOption(user, '資料型態', '下拉單選');
    expect(dialog.getByRole('button', { name: /新增選項/ })).toBeInTheDocument();
    expect(dialog.queryByLabelText('小數位數')).not.toBeInTheDocument();
    expect(dialog.queryByLabelText('最小值')).not.toBeInTheDocument();

    await chooseOption(user, '資料型態', '多行文字');
    expect(dialog.getByLabelText('字元數上限')).toBeInTheDocument();
    expect(dialog.queryByLabelText('格式規則')).not.toBeInTheDocument();
    expect(dialog.queryByRole('button', { name: /新增選項/ })).not.toBeInTheDocument();

    await chooseOption(user, '資料型態', '日期');
    expect(dialog.queryByLabelText('字元數上限')).not.toBeInTheDocument();
    expect(dialog.queryByLabelText('最小值')).not.toBeInTheDocument();
    // 共通設定仍在
    expect(dialog.getByLabelText('預設值')).toBeInTheDocument();
    expect(dialog.getByLabelText('可搜尋')).toBeInTheDocument();
    expect(dialog.getByLabelText('欄寬')).toBeInTheDocument();
  });

  it('validates_field_key_naming_rule', async () => {
    const invoke = renderFieldsPage();
    const { user, dialog } = await openNewFieldDialog();

    const key = dialog.getByLabelText('欄位代碼');
    await user.type(key, 'Qty');
    expect(await dialog.findByText(KEY_MESSAGES.invalidStart)).toBeInTheDocument();
    await user.clear(key);
    await user.type(key, 'unit-price');
    expect(await dialog.findByText(KEY_MESSAGES.invalidChar)).toBeInTheDocument();
    await user.clear(key);
    await user.type(key, 'a'.repeat(31));
    expect(await dialog.findByText(KEY_MESSAGES.tooLong)).toBeInTheDocument();
    await user.clear(key);
    expect(await dialog.findByText(KEY_MESSAGES.empty)).toBeInTheDocument();
    await user.type(key, 'unit_price');
    await waitFor(() => expect(dialog.queryByText(/^代碼/)).not.toBeInTheDocument());

    // 不合法時不送出
    await user.clear(key);
    await user.type(key, '1st');
    await user.type(dialog.getByLabelText('顯示名稱'), '第一');
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));
    expect(await dialog.findByText(KEY_MESSAGES.invalidStart)).toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith('create_field', expect.anything());
  });

  it('creates_select_field', async () => {
    const invoke = renderFieldsPage({
      create_field: () => fieldDef({ field_key: 'status', data_type: 'select' }),
    });
    const { user, dialog } = await openNewFieldDialog();

    await user.type(dialog.getByLabelText('欄位代碼'), 'status');
    await user.type(dialog.getByLabelText('顯示名稱'), '狀態');
    await chooseOption(user, '資料型態', '下拉單選');
    await user.click(dialog.getByRole('button', { name: /新增選項/ }));
    await user.type(dialog.getByLabelText('選項的值'), 'open');
    await user.type(dialog.getByLabelText('選項的名稱'), '處理中');
    await user.click(dialog.getByLabelText('必填'));
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('create_field', {
        key: 'repair',
        req: expect.objectContaining({
          field_key: 'status',
          label: '狀態',
          data_type: 'select',
          is_required: true,
          options: [{ value: 'open', label: '處理中', color: null }],
          decimal_places: null,
          validation: null,
          // 排在現有欄位之後
          sort_order: 2,
        }),
      }),
    );
    // 成功後重新取得欄位列表
    await waitFor(() =>
      expect(invoke.mock.calls.filter(([cmd]) => cmd === 'get_entity')).toHaveLength(2),
    );
  });

  it('creates_relation_field_with_target_entity', async () => {
    const EQUIPMENT = { ...REPAIR, id: 'entity-equipment', key: 'equipment', label: '設備' };
    const PARTS = {
      ...REPAIR,
      id: 'entity-parts',
      key: 'parts',
      label: '更換零件',
      kind: 'detail' as const,
      parent_entity_id: REPAIR.id,
    };
    const invoke = renderFieldsPage({
      list_entities: () => [REPAIR, EQUIPMENT, PARTS],
      create_field: () =>
        fieldDef({
          field_key: 'equipment',
          data_type: 'relation',
          relation_entity_id: EQUIPMENT.id,
        }),
    });
    const { user, dialog } = await openNewFieldDialog();

    await user.type(dialog.getByLabelText('欄位代碼'), 'equipment');
    await user.type(dialog.getByLabelText('顯示名稱'), '設備');
    await chooseOption(user, '資料型態', '關聯');
    // 關聯欄位不可設定預設值
    expect(dialog.queryByLabelText('預設值')).not.toBeInTheDocument();

    // 必須選擇目標；明細子表不能當作目標
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));
    expect(await dialog.findByText('請選擇關聯的資料類別')).toBeInTheDocument();
    await user.click(screen.getByLabelText('關聯的資料類別'));
    expect(screen.queryByTitle('更換零件')).not.toBeInTheDocument();
    await user.click((await screen.findAllByTitle('設備')).at(-1)!);
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('create_field', {
        key: 'repair',
        req: expect.objectContaining({
          field_key: 'equipment',
          data_type: 'relation',
          relation_entity_id: EQUIPMENT.id,
          default_value: null,
        }),
      }),
    );
  });

  it('relation_target_is_locked_when_editing', async () => {
    const EQUIPMENT = { ...REPAIR, id: 'entity-equipment', key: 'equipment', label: '設備' };
    const relation = fieldDef({
      id: 'f-equipment',
      field_key: 'equipment',
      label: '設備',
      data_type: 'relation',
      relation_entity_id: EQUIPMENT.id,
      sort_order: 2,
    });
    renderFieldsPage({
      list_entities: () => [REPAIR, EQUIPMENT],
      get_entity: () => entityDetail([SYSTEM_FIELD, CUSTOM_FIELD, relation]),
    });
    const user = userEvent.setup();
    const row = await rowOf('equipment');
    await user.click(row.getByRole('button', { name: '編輯' }));
    const dialog = within(await screen.findByRole('dialog'));
    expect(dialog.getByLabelText('關聯的資料類別')).toBeDisabled();
    expect(await dialog.findByTitle('設備')).toBeInTheDocument();
  });

  it('system_field_key_and_type_are_not_editable', async () => {
    const invoke = renderFieldsPage({
      update_field: () => ({ ...SYSTEM_FIELD, label: '編號', version: 3 }),
    });
    const user = userEvent.setup();
    const row = await rowOf('record_no');
    await user.click(row.getByRole('button', { name: '編輯' }));
    const dialog = within(await screen.findByRole('dialog'));

    expect(dialog.getByText(/系統欄位只能修改/)).toBeInTheDocument();
    expect(dialog.getByLabelText('欄位代碼')).toBeDisabled();
    expect(dialog.getByLabelText('資料型態')).toBeDisabled();
    // 原本必填，不可取消
    expect(dialog.getByLabelText('必填')).toBeDisabled();
    // 不可修改的屬性不顯示
    expect(dialog.queryByLabelText('唯讀')).not.toBeInTheDocument();
    expect(dialog.queryByLabelText('可搜尋')).not.toBeInTheDocument();
    expect(dialog.queryByLabelText('不可重複')).not.toBeInTheDocument();
    expect(dialog.queryByLabelText('字元數上限')).not.toBeInTheDocument();
    expect(dialog.queryByLabelText('格式規則')).not.toBeInTheDocument();
    // 可以修改的屬性
    expect(dialog.getByLabelText('顯示名稱')).toBeEnabled();
    expect(dialog.getByLabelText('顯示')).toBeEnabled();
    expect(dialog.getByLabelText('欄寬')).toBeEnabled();
    expect(dialog.getByLabelText('說明文字')).toBeEnabled();

    const label = dialog.getByLabelText('顯示名稱');
    await user.clear(label);
    await user.type(label, '編號');
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('update_field', {
        key: 'repair',
        fieldKey: 'record_no',
        req: expect.objectContaining({
          version: SYSTEM_FIELD.version,
          label: '編號',
          data_type: 'text',
          is_unique: true,
          is_searchable: true,
          is_required: true,
          is_active: true,
        }),
      }),
    );
  });

  it('custom_field_key_and_type_are_locked_when_editing', async () => {
    renderFieldsPage();
    const user = userEvent.setup();
    const row = await rowOf('qty');
    await user.click(row.getByRole('button', { name: '編輯' }));
    const dialog = within(await screen.findByRole('dialog'));
    expect(dialog.getByLabelText('欄位代碼')).toBeDisabled();
    expect(dialog.getByLabelText('資料型態')).toBeDisabled();
    expect(dialog.getByLabelText('不可重複')).toBeEnabled();
    expect(dialog.getByLabelText('最小值')).toBeEnabled();
  });

  it('shows_conflict_message_on_409', async () => {
    renderFieldsPage({
      update_field: () =>
        commandError({ kind: 'api', code: 'CONFLICT', message: '此資料已被其他人修改' }),
    });
    const user = userEvent.setup();
    const row = await rowOf('qty');
    await user.click(row.getByRole('button', { name: '編輯' }));
    const dialog = within(await screen.findByRole('dialog'));
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));
    expect(await dialog.findByText(CONFLICT_MESSAGE)).toBeInTheDocument();
  });

  it('shows_validation_error_under_field_on_422', async () => {
    renderFieldsPage({
      create_field: () => validationError({ field: 'field_key' }, '欄位代碼已存在'),
    });
    const { user, dialog } = await openNewFieldDialog();
    await user.type(dialog.getByLabelText('欄位代碼'), 'qty');
    await user.type(dialog.getByLabelText('顯示名稱'), '數量');
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));
    expect(await dialog.findByText('欄位代碼已存在')).toBeInTheDocument();
  });

  it('lists_duplicate_values_when_unique_fails', async () => {
    renderFieldsPage({
      update_field: () =>
        validationError({
          field: 'is_unique',
          total: 2,
          values: [
            { value: 3, count: 2, record_ids: ['r1', 'r2'] },
            { value: 5, count: 3, record_ids: ['r3', 'r4', 'r5'] },
          ],
        }),
    });
    const user = userEvent.setup();
    const row = await rowOf('qty');
    await user.click(row.getByRole('button', { name: '編輯' }));
    const dialog = within(await screen.findByRole('dialog'));
    await user.click(dialog.getByLabelText('不可重複'));
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));
    expect(await dialog.findByText('已有 2 個值重複，無法設為不可重複')).toBeInTheDocument();
    expect(dialog.getByText('：3 筆', { exact: false })).toBeInTheDocument();
  });

  it('deactivates_field_and_keeps_data', async () => {
    const invoke = renderFieldsPage({
      deactivate_field: () => ({ ...CUSTOM_FIELD, is_active: false, version: 5 }),
    });
    const user = userEvent.setup();
    const row = await rowOf('qty');
    await user.click(row.getByRole('button', { name: '停用' }));

    const confirm = await screen.findByRole('tooltip');
    expect(within(confirm).getByText(/資料會保留/)).toBeInTheDocument();
    await user.click(within(confirm).getByRole('button', { name: /停\s*用/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('deactivate_field', {
        key: 'repair',
        fieldKey: 'qty',
        version: CUSTOM_FIELD.version,
      }),
    );
    expect(await screen.findByText('已停用「數量」，資料已保留')).toBeInTheDocument();
  });

  it('reactivates_field', async () => {
    const inactive = fieldDef({ field_key: 'note', label: '備註', is_active: false, version: 6 });
    const invoke = renderFieldsPage({
      get_entity: () => entityDetail([SYSTEM_FIELD, inactive]),
      update_field: () => ({ ...inactive, is_active: true, version: 7 }),
    });
    const user = userEvent.setup();
    const row = await rowOf('note');
    await user.click(row.getByRole('button', { name: '啟用' }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('update_field', {
        key: 'repair',
        fieldKey: 'note',
        req: expect.objectContaining({ version: 6, is_active: true }),
      }),
    );
  });

  it('lists_records_that_failed_type_change', async () => {
    const invoke = renderFieldsPage({
      get_entity: () =>
        entityDetail([SYSTEM_FIELD, fieldDef({ field_key: 'code', label: '代號', version: 2 })]),
      change_field_type: () =>
        validationError(
          {
            field: 'data_type',
            total: 120,
            records: [
              { id: 'rec-1', value: 'abc', deleted: false, message: '不是整數' },
              { id: 'rec-2', value: '1.5', deleted: true, message: '不是整數' },
            ],
          },
          '有 120 筆資料無法轉換',
        ),
    });
    const user = userEvent.setup();
    const row = await rowOf('code');
    await user.click(row.getByRole('button', { name: '變更型態' }));
    const dialog = within(await screen.findByRole('dialog'));
    expect(dialog.getByText('單行文字')).toBeInTheDocument();

    await chooseOption(user, '新的型態', '整數');
    expect(dialog.getByLabelText('最小值')).toBeInTheDocument();
    await user.click(dialog.getByRole('button', { name: /變\s*更/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('change_field_type', {
        key: 'repair',
        fieldKey: 'code',
        req: {
          version: 2,
          data_type: 'integer',
          decimal_places: null,
          options: null,
          validation: null,
        },
      }),
    );
    expect(
      await dialog.findByText('有 120 筆資料無法轉換，型態未變更，以下列出前 2 筆'),
    ).toBeInTheDocument();
    const failed = within(dialog.getByText('rec-1').closest('tr')!);
    expect(failed.getByText('abc')).toBeInTheDocument();
    expect(failed.getByText('不是整數')).toBeInTheDocument();
    const deleted = within(dialog.getByText('rec-2').closest('tr')!);
    expect(deleted.getByText('已刪除')).toBeInTheDocument();
    expect(deleted.getByText('1.5')).toBeInTheDocument();
    // 失敗時不關閉
    expect(screen.getByRole('dialog')).toBeInTheDocument();
  });

  it('reorders_fields_by_keyboard_drag', async () => {
    const reordered: FieldDefinition[] = [
      { ...CUSTOM_FIELD, sort_order: 0, version: 5 },
      { ...SYSTEM_FIELD, sort_order: 1, version: 3 },
    ];
    const invoke = renderFieldsPage({ reorder_fields: () => reordered });
    const user = userEvent.setup();
    await rowOf('record_no');

    await dragDownOneRow(user, '單號');

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('reorder_fields', {
        key: 'repair',
        req: {
          fields: [
            { field_key: 'qty', version: CUSTOM_FIELD.version },
            { field_key: 'record_no', version: SYSTEM_FIELD.version },
          ],
        },
      }),
    );
  });

  it('reorder_conflict_shows_message', async () => {
    renderFieldsPage({
      reorder_fields: () => commandError({ kind: 'api', code: 'CONFLICT', message: 'x' }),
    });
    const user = userEvent.setup();
    await rowOf('record_no');
    await dragDownOneRow(user, '單號');
    expect(await screen.findByText(CONFLICT_MESSAGE)).toBeInTheDocument();
  });
});

describe('明細子表', () => {
  const LINES = {
    ...REPAIR,
    id: 'entity-repair-lines',
    key: 'repair_lines',
    label: '維修明細',
    kind: 'detail' as const,
    parent_entity_id: REPAIR.id,
    is_active: false,
  };

  it('lists_and_creates_detail_entities', async () => {
    const created = { ...LINES, id: 'entity-parts', key: 'parts', label: '更換零件' };
    let all = [REPAIR, LINES];
    const invoke = renderFieldsPage({
      list_all_entities: () => all,
      create_entity: () => {
        all = [...all, created];
        return { entity: created, fields: [] };
      },
    });
    const user = userEvent.setup();
    const card = within(
      (
        await screen.findByText(
          'repair_lines',
          { selector: 'td code' },
          { timeout: PAGE_LOAD_TIMEOUT },
        )
      ).closest('.ant-card')! as HTMLElement,
    );
    expect(card.getByText('已停用')).toBeInTheDocument();
    expect(card.getByRole('button', { name: '啟用' })).toBeInTheDocument();

    await user.click(card.getByRole('button', { name: /新增明細子表/ }));
    const dialog = within(await screen.findByRole('dialog'));
    expect(dialog.getByText('新增明細子表：維修單')).toBeInTheDocument();
    // 明細子表不可設定單號規則
    expect(dialog.queryByLabelText('單號規則')).not.toBeInTheDocument();
    await user.type(dialog.getByLabelText('代碼'), 'parts');
    await user.type(dialog.getByLabelText('名稱'), '更換零件');
    await user.click(dialog.getByRole('button', { name: /建\s*立/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('create_entity', {
        req: {
          key: 'parts',
          label: '更換零件',
          icon: null,
          kind: 'detail',
          parent_entity_id: REPAIR.id,
          numbering_pattern: null,
          title_template: null,
          sort_order: 1,
        },
      }),
    );
    await waitFor(() => expect(window.location.hash).toBe('#/schema/parts'));
  });

  it('detail_page_returns_to_parent_without_detail_section', async () => {
    renderFieldsPage({
      list_all_entities: () => [REPAIR, LINES],
      get_entity: () => ({ entity: LINES, fields: [CUSTOM_FIELD] }),
    });
    const user = userEvent.setup();
    await rowOf('qty');
    expect(screen.queryByText('新增明細子表')).not.toBeInTheDocument();
    await user.click(await screen.findByRole('button', { name: '回到「維修單」' }));
    await waitFor(() => expect(window.location.hash).toBe('#/schema/repair'));
  });
});

describe('公式欄位', () => {
  /** 模擬 `check_formula_syntax`：以 `*` 結尾視為不完整，其他都正確。 */
  const checkSyntax: CommandHandler = ({ formula }) => {
    const text = String(formula);
    if (text.trimEnd().endsWith('*')) {
      const end = Array.from(text).length;
      return { message: '公式不完整', start: end, end };
    }
    return null;
  };

  /** 新增欄位對話框，選好「公式」型態。 */
  async function openNewFormulaDialog() {
    const opened = await openNewFieldDialog();
    await chooseOption(opened.user, '資料型態', '公式');
    return opened;
  }

  /** 輸入公式（`{` 在 user-event 是特殊字元，用貼上）。 */
  async function enterFormula(
    user: ReturnType<typeof userEvent.setup>,
    dialog: ReturnType<typeof within>,
    text: string,
  ) {
    const input = dialog.getByLabelText('公式');
    await user.clear(input);
    await user.click(input);
    await user.paste(text);
  }

  it('creates_formula_field_with_correct_payload', async () => {
    const invoke = renderFieldsPage({
      check_formula_syntax: checkSyntax,
      create_field: () => fieldDef({ field_key: 'total', data_type: 'formula' }),
    });
    const { user, dialog } = await openNewFormulaDialog();

    // 公式欄位不顯示不可重複、預設值、必填
    expect(dialog.queryByLabelText('不可重複')).not.toBeInTheDocument();
    expect(dialog.queryByLabelText('預設值')).not.toBeInTheDocument();
    expect(dialog.queryByLabelText('必填')).not.toBeInTheDocument();
    expect(dialog.getByLabelText('可搜尋')).toBeInTheDocument();

    await user.type(dialog.getByLabelText('欄位代碼'), 'total');
    await user.type(dialog.getByLabelText('顯示名稱'), '總價');
    await enterFormula(user, dialog, ' {qty} * 1.05 ');
    // 小數位數只在結果型態是小數、金額時出現
    expect(dialog.queryByLabelText('小數位數')).not.toBeInTheDocument();
    await chooseOption(user, '結果型態', '金額');
    await user.type(dialog.getByLabelText('小數位數'), '2');
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('create_field', {
        key: 'repair',
        req: expect.objectContaining({
          field_key: 'total',
          label: '總價',
          data_type: 'formula',
          formula: '{qty} * 1.05',
          formula_result_type: 'money',
          decimal_places: 2,
          is_required: false,
          is_unique: false,
          default_value: null,
          options: null,
          validation: null,
          relation_entity_id: null,
        }),
      }),
    );
  });

  it('shows_syntax_error_and_disables_submit', async () => {
    const invoke = renderFieldsPage({ check_formula_syntax: checkSyntax });
    const { user, dialog } = await openNewFormulaDialog();
    const save = dialog.getByRole('button', { name: /儲\s*存/ });

    await enterFormula(user, dialog, '{qty} *');
    const alert = await dialog.findByRole('alert');
    expect(within(alert).getByText('第 8 個字元：公式不完整')).toBeInTheDocument();
    // 錯誤位置在結尾：以標示顯示
    expect(
      within(alert).getByTestId('formula-error-position').querySelector('mark'),
    ).not.toBeNull();
    await waitFor(() => expect(save).toBeDisabled());
    expect(invoke).toHaveBeenCalledWith('check_formula_syntax', { formula: '{qty} *' });

    // 修正後恢復
    await user.type(dialog.getByLabelText('公式'), ' 2');
    await waitFor(() => expect(dialog.queryByRole('alert')).not.toBeInTheDocument());
    expect(save).toBeEnabled();
    expect(invoke).not.toHaveBeenCalledWith('create_field', expect.anything());
  });

  it('shows_server_cycle_error_under_formula', async () => {
    const message = '公式有循環引用：a → b → a';
    renderFieldsPage({
      check_formula_syntax: checkSyntax,
      create_field: () => validationError({ field: 'formula', cycle: ['a', 'b', 'a'] }, message),
    });
    const { user, dialog } = await openNewFormulaDialog();

    await user.type(dialog.getByLabelText('欄位代碼'), 'a');
    await user.type(dialog.getByLabelText('顯示名稱'), '甲');
    await enterFormula(user, dialog, '{b} + 1');
    await chooseOption(user, '結果型態', '整數');
    await user.click(dialog.getByRole('button', { name: /儲\s*存/ }));

    const error = await dialog.findByText(message);
    // 顯示在公式輸入區的表單項目下方
    expect(error.closest('.ant-form-item')).toContainElement(dialog.getByLabelText('公式'));
  });

  it('inserts_field_code_from_list', async () => {
    const LINES = {
      ...REPAIR,
      id: 'entity-lines',
      key: 'lines',
      label: '明細',
      kind: 'detail' as const,
      parent_entity_id: REPAIR.id,
    };
    const amount = fieldDef({
      id: 'f-amount',
      entity_id: LINES.id,
      field_key: 'amount',
      label: '金額',
      data_type: 'money',
      decimal_places: 2,
    });
    renderFieldsPage({
      check_formula_syntax: checkSyntax,
      list_all_entities: () => [REPAIR, LINES],
      get_entity: ({ key }) =>
        key === 'lines' ? { entity: LINES, fields: [amount] } : entityDetail(),
    });
    const { user, dialog } = await openNewFormulaDialog();

    // 清單顯示名稱，插入的是代碼
    await chooseOption(user, '插入欄位', '數量');
    expect(dialog.getByLabelText('公式')).toHaveValue('{qty}');

    await user.type(dialog.getByLabelText('公式'), ' + ');
    await chooseOption(user, '插入欄位', '金額（合計）');
    expect(dialog.getByLabelText('公式')).toHaveValue('{qty} + SUM(lines.amount)');
  });

  it('lists_formulas_when_deactivating_referenced_field', async () => {
    renderFieldsPage({
      deactivate_field: () =>
        validationError(
          {
            field: 'formula',
            formulas: [
              {
                entity_key: 'repair',
                entity_label: '維修單',
                field_key: 'total',
                field_label: '總價',
              },
            ],
          },
          '欄位被公式引用，無法停用',
        ),
    });
    const user = userEvent.setup();
    const row = await rowOf('qty');
    await user.click(row.getByRole('button', { name: '停用' }));
    const confirm = await screen.findByRole('tooltip');
    await user.click(within(confirm).getByRole('button', { name: /停\s*用/ }));

    const dialog = within(await screen.findByRole('dialog'));
    // antd 的確認對話框標題也出現在 aria 標籤中
    expect(dialog.getAllByText('無法停用「數量」').length).toBeGreaterThan(0);
    expect(dialog.getByText(/以下 1 個公式欄位引用了這個欄位/)).toBeInTheDocument();
    expect(dialog.getByText('總價')).toBeInTheDocument();
    expect(dialog.getByText('total')).toBeInTheDocument();
  });

  it('shows_formula_result_type_in_list', async () => {
    renderFieldsPage({
      get_entity: () =>
        entityDetail([
          CUSTOM_FIELD,
          fieldDef({
            field_key: 'total',
            label: '總價',
            data_type: 'formula',
            formula: '{qty} * 2',
            formula_result_type: 'integer',
            sort_order: 2,
          }),
        ]),
    });
    const row = await rowOf('total');
    expect(row.getByText('公式（整數）')).toBeInTheDocument();
  });
});
