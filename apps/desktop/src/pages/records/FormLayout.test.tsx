import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type {
  EntityDefinition,
  FormLayoutConfig_Serialize as FormLayoutConfig,
} from '../../bindings';
import { ADMIN, startupCommands } from '../../test/fixtures';
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

const PARTS: EntityDefinition = {
  ...REPAIR,
  id: '0196f5a0-0000-7000-8000-000000000110',
  key: 'parts',
  label: '零件',
  kind: 'detail',
  parent_entity_id: REPAIR.id,
  sort_order: 1,
};

const NAME = fieldDef({ field_key: 'name', label: '名稱', is_required: true, sort_order: 0 });
const PHONE = fieldDef({ field_key: 'phone', label: '電話', sort_order: 1 });
const ADDRESS = fieldDef({ field_key: 'address', label: '地址', sort_order: 2 });
const NOTE = fieldDef({ field_key: 'note', label: '備註', data_type: 'long_text', sort_order: 3 });
const SECRET = fieldDef({ field_key: 'secret', label: '隱藏欄位', is_visible: false });
const PART = fieldDef({ entity_id: PARTS.id, field_key: 'part', label: '品名' });

function renderAt(
  layout: FormLayoutConfig | undefined,
  extra: Record<string, CommandHandler> = {},
): ReturnType<typeof mockCommands> {
  window.location.hash = '#/entities/repair/new';
  const invoke = mockCommands({
    ...startupCommands(ADMIN),
    list_entities: () => [ORDER, PARTS],
    get_entity: ({ key }) =>
      key === 'parts'
        ? { entity: PARTS, fields: [PART] }
        : { entity: ORDER, fields: [NAME, PHONE, ADDRESS, NOTE, SECRET] },
    get_form_layout: () =>
      layout
        ? {
            id: '0196f5a0-0000-7000-8000-000000000301',
            version: 1,
            is_default: false,
            config: layout,
          }
        : commandError({ kind: 'network', message: '無法連線' }),
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

async function waitForForm() {
  return screen.findByRole('textbox', { name: '名稱' }, { timeout: PAGE_LOAD_TIMEOUT });
}

/** 依 DOM 順序列出版面中的項目（欄位代碼或系統區塊代碼）。 */
function itemOrder(root: ParentNode = document): string[] {
  return Array.from(root.querySelectorAll<HTMLElement>('[data-item]')).map(
    (el) => el.dataset.item!,
  );
}

function cellOf(item: string): HTMLElement {
  return document.querySelector<HTMLElement>(`[data-item="${item}"]`)!;
}

const LAYOUT: FormLayoutConfig = {
  sections: [
    {
      key: 'basic',
      title: '基本資料',
      columns: 2,
      collapsible: false,
      collapsed: false,
      items: [
        { type: 'field', field: 'name', span: 1 },
        { type: 'block', block: 'record_no' },
        { type: 'field', field: 'address', span: 2 },
        { type: 'spacer', span: 1 },
        { type: 'field', field: 'phone', span: 1 },
        { type: 'field', field: 'secret', span: 1 },
      ],
    },
    {
      key: 'lines',
      title: '',
      columns: 1,
      collapsible: false,
      collapsed: false,
      items: [{ type: 'block', block: 'lines:parts' }],
    },
    {
      key: 'extra',
      title: '補充說明',
      columns: 1,
      collapsible: true,
      collapsed: true,
      items: [{ type: 'field', field: 'note', span: 1 }],
    },
  ],
};

describe('表單版面', () => {
  it('lays_out_columns_spans_and_spacers', async () => {
    renderAt(LAYOUT);
    await waitForForm();

    const basic = screen.getByRole('region', { name: '基本資料' });
    expect(within(basic).getByRole('heading', { name: '基本資料' })).toBeInTheDocument();
    const grid = basic.querySelector<HTMLElement>('.form-layout-grid')!;
    expect(grid).toHaveClass('form-layout-cols-2');
    expect(grid.style.getPropertyValue('--columns')).toBe('2');

    // 版面順序；不顯示的欄位略過
    expect(itemOrder(basic)).toEqual(['name', 'record_no', 'address', 'phone']);
    expect(cellOf('address').style.getPropertyValue('--span')).toBe('2');
    expect(cellOf('name').style.getPropertyValue('--span')).toBe('1');
    expect(basic.querySelectorAll('.form-layout-spacer')).toHaveLength(1);
    expect(screen.queryByText('隱藏欄位')).not.toBeInTheDocument();

    // 單號佔一格，明細表佔滿整列
    expect(cellOf('record_no')).not.toHaveClass('form-layout-block');
    expect(cellOf('lines:parts')).toHaveClass('form-layout-block');
    expect(screen.getByRole('textbox', { name: '單號' })).toHaveAttribute(
      'placeholder',
      '存檔後產生',
    );
  });

  it('system_blocks_follow_the_layout', async () => {
    renderAt({
      sections: [
        {
          key: 'top',
          title: '',
          columns: 1,
          collapsible: false,
          collapsed: false,
          items: [
            { type: 'block', block: 'lines:parts' },
            { type: 'field', field: 'name', span: 1 },
          ],
        },
      ],
    });
    await waitForForm();

    // 明細表在名稱之前；沒有放進版面的欄位與單號依規則附加
    expect(itemOrder()).toEqual(['record_no', 'lines:parts', 'name', 'phone', 'address', 'note']);
    const other = screen.getByRole('region', { name: '其他欄位' });
    expect(itemOrder(other)).toEqual(['phone', 'address', 'note']);
  });

  it('tab_order_follows_reading_order', async () => {
    const twoColumns: FormLayoutConfig = {
      sections: [
        {
          key: 'basic',
          title: '',
          columns: 2,
          collapsible: false,
          collapsed: false,
          items: [
            { type: 'field', field: 'address', span: 1 },
            { type: 'field', field: 'name', span: 1 },
            { type: 'field', field: 'phone', span: 2 },
          ],
        },
      ],
    };
    renderAt(twoColumns);
    await waitForForm();
    const user = userEvent.setup();

    await user.click(screen.getByRole('textbox', { name: '地址' }));
    await user.tab();
    expect(screen.getByRole('textbox', { name: '名稱' })).toHaveFocus();
    await user.tab();
    expect(screen.getByRole('textbox', { name: '電話' })).toHaveFocus();
  });

  it('collapsed_section_opens_on_click_and_on_validation_error', async () => {
    const layout: FormLayoutConfig = {
      sections: [
        {
          key: 'basic',
          title: '基本資料',
          columns: 1,
          collapsible: false,
          collapsed: false,
          items: [{ type: 'field', field: 'phone', span: 1 }],
        },
        {
          key: 'more',
          title: '其他資料',
          columns: 2,
          collapsible: true,
          collapsed: true,
          items: [
            { type: 'field', field: 'name', span: 1 },
            { type: 'field', field: 'address', span: 1 },
          ],
        },
      ],
    };
    renderAt(layout);
    // 收折時欄位仍在表單中，只是隱藏
    const name = await screen.findByLabelText('名稱', {}, { timeout: PAGE_LOAD_TIMEOUT });
    expect(name).not.toBeVisible();
    const user = userEvent.setup();

    // 必填的名稱在收折的區塊內：送出時自動展開並顯示錯誤
    await user.click(screen.getByRole('button', { name: /儲存/ }));
    await waitFor(() => expect(name).toBeVisible());
    expect(await screen.findByText('此欄位為必填')).toBeInTheDocument();

    // 點標題可以收折
    await user.click(screen.getByRole('button', { name: /其他資料/ }));
    await waitFor(() => expect(name).not.toBeVisible());
    await user.click(screen.getByRole('button', { name: /其他資料/ }));
    await waitFor(() => expect(name).toBeVisible());
  });

  it('falls_back_to_generated_layout_when_loading_fails', async () => {
    renderAt(undefined);
    await waitForForm();
    expect(itemOrder()).toEqual(['record_no', 'name', 'phone', 'address', 'note', 'lines:parts']);
    expect(document.querySelector('.form-layout-cols-1')).not.toBeNull();
  });
});
