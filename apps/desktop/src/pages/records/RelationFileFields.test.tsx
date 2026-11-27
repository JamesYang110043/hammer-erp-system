import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { Channel } from '@tauri-apps/api/core';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type {
  Attachment_Serialize as Attachment,
  CurrentUser,
  Page,
  Record as DataRecord,
  UploadProgress,
} from '../../bindings';
import { ADMIN, VIEWER, startupCommands } from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import { REPAIR, entityDetail, fieldDef } from '../../test/schemaFixtures';

const PAGE_LOAD_TIMEOUT = 5000;

const EQUIPMENT = { ...REPAIR, id: 'entity-equipment', key: 'equipment', label: '設備' };

const NAME = fieldDef({ field_key: 'name', label: '名稱', sort_order: 0 });
const EQUIP = fieldDef({
  field_key: 'equipment',
  label: '設備',
  data_type: 'relation',
  relation_entity_id: EQUIPMENT.id,
  sort_order: 1,
});
const PHOTO = fieldDef({ field_key: 'photo', label: '照片', data_type: 'file', sort_order: 2 });
const FIELDS = [NAME, EQUIP, PHOTO];

const E1 = '0196f5a0-0000-7000-8000-000000000501';
const E2 = '0196f5a0-0000-7000-8000-000000000502';
const RECORD_ID = '0196f5a0-0000-7000-8000-000000000201';

function attachment(patch: Partial<Attachment> = {}): Attachment {
  return {
    id: '0196f5a0-0000-7000-8000-000000000601',
    file_name: '維修報告.pdf',
    mime_type: 'application/pdf',
    size_bytes: 1536,
    sha256: 'abc',
    created_at: '2026-11-16T01:00:00Z',
    ...patch,
  };
}

function record(patch: Partial<DataRecord> = {}): DataRecord {
  const file = attachment();
  return {
    id: RECORD_ID,
    record_no: null,
    title: '冷氣維修',
    data: { name: '冷氣維修', equipment: E1, photo: file.id },
    relations: { equipment: { id: E1, title: '冷氣 A', deleted: true, inactive: false } },
    attachments: { photo: file },
    created_at: '2026-11-13T01:00:00Z',
    created_by: null,
    updated_at: '2026-11-13T01:00:00Z',
    updated_by: null,
    version: 3,
    ...patch,
  };
}

/** 選擇器搜尋：沒有關鍵字時回傳兩筆，有關鍵字時回傳標題包含關鍵字的資料。 */
const lookup: CommandHandler = (args) => {
  const q = (args.query as { q: string | null }).q ?? '';
  return [
    { id: E1, title: '冷氣 A' },
    { id: E2, title: '冷氣 B' },
  ].filter((i) => i.title.includes(q));
};

function renderAt(
  hash: string,
  extra: Record<string, CommandHandler> = {},
  user: CurrentUser = ADMIN,
): ReturnType<typeof mockCommands> {
  window.location.hash = hash;
  const invoke = mockCommands({
    ...startupCommands(user),
    list_entities: () => [REPAIR, EQUIPMENT],
    get_entity: () => entityDetail(FIELDS),
    get_record: () => record(),
    lookup_records: lookup,
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

function calls(invoke: ReturnType<typeof mockCommands>, cmd: string) {
  return invoke.mock.calls.filter(([c]) => c === cmd).map(([, args]) => args);
}

async function waitForForm() {
  return screen.findByRole('textbox', { name: '名稱' }, { timeout: PAGE_LOAD_TIMEOUT });
}

/** 欄位的 Form.Item。 */
function formItem(label: string) {
  const el = document.querySelector<HTMLElement>(`label[title="${label}"]`);
  if (!el) {
    throw new Error(`找不到欄位「${label}」`);
  }
  return within(el.closest<HTMLElement>('.ant-form-item')!);
}

/** antd Select 的輸入框（`aria-label` 也會出現在下拉清單上）。 */
function combobox(label: string): HTMLElement {
  return screen.getAllByLabelText(label).find((el) => el.getAttribute('role') === 'combobox')!;
}

describe('關聯選擇器', () => {
  it('searches_with_debounce_and_selects_record', async () => {
    const invoke = renderAt('#/entities/repair/new', {
      create_record: () => record({ version: 1 }),
    });
    const user = userEvent.setup();
    await user.type(await waitForForm(), '冷氣維修');

    // 打開選單時列出最新的資料
    await user.click(combobox('設備'));
    expect(await screen.findByTitle('冷氣 A')).toBeInTheDocument();
    expect(calls(invoke, 'lookup_records')[0]).toEqual({
      key: 'equipment',
      query: { q: null, limit: 20 },
    });

    // 輸入時延遲查詢：停頓後只以完整的關鍵字查詢一次
    await user.type(combobox('設備'), '冷氣 B');
    await waitFor(() => expect(screen.queryByTitle('冷氣 A')).not.toBeInTheDocument());
    const searched = calls(invoke, 'lookup_records')
      .map((a) => (a.query as { q: string | null }).q)
      .filter((q) => q !== null);
    expect(searched).toEqual(['冷氣 B']);

    await user.click(screen.getByTitle('冷氣 B'));
    expect(formItem('設備').getByTitle('冷氣 B')).toHaveClass('ant-select-selection-item');
    await user.click(screen.getByRole('button', { name: /儲\s*存/ }));

    await waitFor(() => expect(calls(invoke, 'create_record')).toHaveLength(1));
    const req = calls(invoke, 'create_record')[0]!.req as { data: Record<string, unknown> };
    expect(req.data).toEqual({ name: '冷氣維修', equipment: E2, photo: null });
  });

  it('shows_deleted_relation_and_can_clear_it', async () => {
    const invoke = renderAt('#/entities/repair/' + RECORD_ID, {
      update_record: () => record({ version: 4 }),
    });
    const user = userEvent.setup();
    await waitForForm();

    expect(formItem('設備').getByTitle('冷氣 A（已刪除）')).toHaveClass(
      'ant-select-selection-item',
    );

    // 清除關聯
    const clear = formItem('設備').getByLabelText('close-circle');
    await user.click(clear);
    await user.click(screen.getByRole('button', { name: /儲\s*存/ }));
    await waitFor(() => expect(calls(invoke, 'update_record')).toHaveLength(1));
    expect(calls(invoke, 'update_record')[0]!.req).toEqual({
      version: 3,
      data: { equipment: null },
    });
  });
});

describe('附件欄位', () => {
  it('uploads_with_progress_and_saves_attachment_id', async () => {
    const uploaded = attachment({ id: '0196f5a0-0000-7000-8000-000000000602', size_bytes: 2048 });
    let finish: (value: Attachment) => void = () => {};
    const invoke = renderAt('#/entities/repair/new', {
      upload_attachment: (args) => {
        const channel = args.onProgress as Channel<UploadProgress>;
        channel.onmessage({ file_name: '報價單 中文.pdf', sent_bytes: 0, total_bytes: 2048 });
        channel.onmessage({ file_name: '報價單 中文.pdf', sent_bytes: 1024, total_bytes: 2048 });
        return new Promise<Attachment>((resolve) => {
          finish = resolve;
        });
      },
      create_record: () => record({ version: 1 }),
    });
    const user = userEvent.setup();
    await user.type(await waitForForm(), '冷氣維修');

    await user.click(formItem('照片').getByRole('button', { name: /上傳檔案/ }));
    // 上傳中顯示檔名與進度
    expect(await formItem('照片').findByText('報價單 中文.pdf')).toBeInTheDocument();
    expect(formItem('照片').getByText('50%')).toBeInTheDocument();
    expect(formItem('照片').getByRole('button', { name: /上傳中/ })).toBeInTheDocument();

    finish({ ...uploaded, file_name: '報價單 中文.pdf' });
    // 完成後顯示檔名與大小
    expect(await formItem('照片').findByRole('button', { name: /報價單 中文\.pdf/ })).toBeVisible();
    expect(formItem('照片').getByText('2 KB')).toBeInTheDocument();
    expect(formItem('照片').getByRole('button', { name: /更換檔案/ })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: /儲\s*存/ }));
    await waitFor(() => expect(calls(invoke, 'create_record')).toHaveLength(1));
    const req = calls(invoke, 'create_record')[0]!.req as { data: Record<string, unknown> };
    expect(req.data.photo).toBe(uploaded.id);
  });

  it('removes_attachment_and_saves_null', async () => {
    const invoke = renderAt('#/entities/repair/' + RECORD_ID, {
      update_record: () => record({ version: 4 }),
    });
    const user = userEvent.setup();
    await waitForForm();

    expect(formItem('照片').getByRole('button', { name: /維修報告\.pdf/ })).toBeInTheDocument();
    expect(formItem('照片').getByText('1.5 KB')).toBeInTheDocument();

    await user.click(formItem('照片').getByRole('button', { name: /移\s*除/ }));
    expect(formItem('照片').queryByText('維修報告.pdf')).not.toBeInTheDocument();
    expect(formItem('照片').getByRole('button', { name: /上傳檔案/ })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: /儲\s*存/ }));
    await waitFor(() => expect(calls(invoke, 'update_record')).toHaveLength(1));
    expect(calls(invoke, 'update_record')[0]!.req).toEqual({ version: 3, data: { photo: null } });
  });

  it('shows_upload_errors_and_ignores_cancel', async () => {
    let result: () => unknown = () => null;
    const invoke = renderAt('#/entities/repair/new', { upload_attachment: () => result() });
    const user = userEvent.setup();
    await waitForForm();

    // 使用者取消選檔：沒有變化
    await user.click(formItem('照片').getByRole('button', { name: /上傳檔案/ }));
    await waitFor(() => expect(calls(invoke, 'upload_attachment')).toHaveLength(1));
    expect(formItem('照片').getByRole('button', { name: /上傳檔案/ })).toBeEnabled();
    expect(screen.queryByText(/上傳失敗/)).not.toBeInTheDocument();

    // 超過大小上限
    result = () =>
      commandError({
        kind: 'api',
        code: 'PAYLOAD_TOO_LARGE',
        message: '檔案超過大小上限 20 MB',
        details: null,
      });
    await user.click(formItem('照片').getByRole('button', { name: /上傳檔案/ }));
    expect(await screen.findByText('上傳失敗：檔案超過大小上限 20 MB')).toBeInTheDocument();
    expect(formItem('照片').queryByRole('button', { name: /移\s*除/ })).not.toBeInTheDocument();
  });

  it('viewer_can_download_but_not_upload', async () => {
    const invoke = renderAt(
      '#/entities/repair/' + RECORD_ID,
      // 使用者在另存新檔對話框改了檔名
      { download_attachment: () => '維修報告_下載.pdf' },
      VIEWER,
    );
    const user = userEvent.setup();
    await waitForForm();

    expect(formItem('照片').getByRole('button', { name: /更換檔案/ })).toBeDisabled();
    expect(formItem('照片').getByRole('button', { name: /移\s*除/ })).toBeDisabled();
    const link = formItem('照片').getByRole('button', { name: /維修報告\.pdf/ });
    expect(link).toBeEnabled();
    await user.click(link);

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('download_attachment', {
        id: attachment().id,
        fileName: '維修報告.pdf',
      }),
    );
    // 成功訊息顯示實際存檔的檔名，不是附件原始檔名
    expect(await screen.findByText('已儲存「維修報告_下載.pdf」')).toBeInTheDocument();
  });
});

describe('列表', () => {
  const GENERATED = {
    source: 'generated',
    layout: null,
    config: {
      columns: ['name', 'equipment', 'photo'].map((k) => ({ field_key: k, width: null })),
      sort: null,
      filters: [],
    },
  };
  const page = (items: DataRecord[]): Page<DataRecord> => ({
    items,
    total: items.length,
    page: 1,
    page_size: 50,
  });

  function renderList(extra: Record<string, CommandHandler> = {}) {
    return renderAt('#/entities/repair', {
      get_default_layout: () => GENERATED,
      list_layouts: () => [],
      list_records: () =>
        page([
          record({
            relations: { equipment: { id: E1, title: '冷氣 A', deleted: false, inactive: false } },
          }),
        ]),
      ...extra,
    });
  }

  async function firstRow() {
    const cell = await screen.findByText(
      '冷氣維修',
      { selector: 'td' },
      { timeout: PAGE_LOAD_TIMEOUT },
    );
    return within(cell.closest('tr')!);
  }

  it('shows_relation_title_and_downloadable_file_name', async () => {
    const invoke = renderList({ download_attachment: () => null });
    const user = userEvent.setup();
    const row = await firstRow();

    expect(row.getByText('冷氣 A')).toBeInTheDocument();
    // 關聯、附件欄位不能排序
    const header = (label: string) =>
      Array.from(document.querySelectorAll('.ant-card thead th')).find(
        (th) => th.textContent === label,
      )!;
    expect(header('設備')).not.toHaveClass('ant-table-column-has-sorters');
    expect(header('名稱')).toHaveClass('ant-table-column-has-sorters');

    // 點檔名下載，不會開啟資料
    await user.click(row.getByRole('button', { name: /維修報告\.pdf/ }));
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('download_attachment', {
        id: attachment().id,
        fileName: '維修報告.pdf',
      }),
    );
    expect(window.location.hash).toBe('#/entities/repair');
    // 取消另存新檔時不顯示成功訊息
    expect(screen.queryByText(/已儲存/)).not.toBeInTheDocument();
  });

  it('filters_relation_by_record_and_file_by_empty', async () => {
    const invoke = renderList();
    const user = userEvent.setup();
    await firstRow();

    const choose = async (label: string, option: string, index = 0) => {
      await user.click(
        screen.getAllByLabelText(label).filter((el) => el.getAttribute('role') === 'combobox')[
          index
        ]!,
      );
      await user.click((await screen.findAllByTitle(option)).at(-1)!);
    };

    // 設備等於「冷氣 B」
    await user.click(screen.getByRole('button', { name: /新增條件/ }));
    await choose('篩選欄位', '設備');
    await choose('設備的篩選值', '冷氣 B');

    // 照片：只有「沒有值」、「有值」
    await user.click(screen.getByRole('button', { name: /新增條件/ }));
    await choose('篩選欄位', '照片', 1);
    await user.click(
      screen
        .getAllByLabelText('篩選條件')
        .filter((el) => el.getAttribute('role') === 'combobox')[1]!,
    );
    const opOptions = Array.from(
      document.querySelectorAll('.ant-select-dropdown:not(.ant-select-dropdown-hidden)'),
    )
      .at(-1)!
      .querySelectorAll('.ant-select-item-option');
    expect(Array.from(opOptions).map((o) => o.getAttribute('title'))).toEqual(['沒有值', '有值']);
    await user.click((await screen.findAllByTitle('有值')).at(-1)!);

    await user.click(screen.getByRole('button', { name: /套用篩選/ }));
    await waitFor(() =>
      expect(calls(invoke, 'list_records').at(-1)!.query).toEqual({
        page: 1,
        page_size: 50,
        sort: null,
        filters: [
          { field_key: 'equipment', op: 'eq', value: E2 },
          { field_key: 'photo', op: 'empty', value: 'false' },
        ],
      }),
    );
  });
});
