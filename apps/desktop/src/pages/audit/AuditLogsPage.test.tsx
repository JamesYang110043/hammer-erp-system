import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import type { AuditLogEntry, Page } from '../../bindings';
import { ADMIN, managedUser, startupCommands } from '../../test/fixtures';
import { mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';

const EDITOR = managedUser();

function entry(patch: Partial<AuditLogEntry> = {}): AuditLogEntry {
  return {
    id: '0196f5a0-0000-7000-8000-00000000f001',
    // 台北時間 2026-11-13 09:00:00
    occurred_at: '2026-11-13T01:00:00Z',
    user_id: ADMIN.id,
    username: 'admin',
    user_display_name: '王管理',
    action: 'update',
    entity_key: 'user',
    record_id: EDITOR.id,
    before_data: { display_name: '陳編輯' },
    after_data: { display_name: '陳大編' },
    client_info: 'HammerERP/0.1.0',
    ...patch,
  };
}

function page(items: AuditLogEntry[], total = items.length, p = 1, size = 50): Page<AuditLogEntry> {
  return { items, total, page: p, page_size: size };
}

const EMPTY_QUERY = {
  from: null,
  to: null,
  user_id: null,
  action: null,
  entity_key: null,
  record_id: null,
  page: 1,
  page_size: 50,
};

function renderAuditPage(extra: Record<string, CommandHandler> = {}) {
  window.location.hash = '#/audit-logs';
  const invoke = mockCommands({
    ...startupCommands(ADMIN),
    list_users: () => [EDITOR],
    list_audit_logs: () => page([entry()]),
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

function auditQueries(invoke: ReturnType<typeof mockCommands>) {
  return invoke.mock.calls
    .filter(([cmd]) => cmd === 'list_audit_logs')
    .map(([, args]) => args.query);
}

describe('操作紀錄頁', () => {
  it('initial_load_sends_default_paging', async () => {
    const invoke = renderAuditPage();

    const cell = await screen.findByRole('cell', { name: '2026-11-13 09:00:00' });
    const row = within(cell.closest('tr')!);
    expect(row.getByText('王管理（admin）')).toBeInTheDocument();
    expect(row.getByText('修改')).toBeInTheDocument();
    expect(row.getByText('帳號')).toBeInTheDocument();
    expect(auditQueries(invoke)).toEqual([EMPTY_QUERY]);
  });

  it('filters_are_sent_as_query_params', async () => {
    const invoke = renderAuditPage();
    const user = userEvent.setup();
    await screen.findByRole('cell', { name: '2026-11-13 09:00:00' });

    // 日期區間
    await user.click(screen.getByPlaceholderText('開始日期'));
    await user.type(screen.getByPlaceholderText('開始日期'), '2026-11-01{Enter}');
    await user.type(screen.getByPlaceholderText('結束日期'), '2026-11-03{Enter}');
    // 使用者
    await user.click(screen.getByLabelText('使用者'));
    await user.click(await screen.findByTitle('陳編輯（editor1）'));
    // 動作
    await user.click(screen.getByLabelText('動作'));
    await user.click(await screen.findByTitle('變更密碼'));

    await user.click(screen.getByRole('button', { name: /查\s*詢/ }));

    await waitFor(() =>
      expect(auditQueries(invoke).at(-1)).toEqual({
        ...EMPTY_QUERY,
        from: '2026-11-01T00:00:00+08:00',
        to: '2026-11-04T00:00:00+08:00',
        user_id: EDITOR.id,
        action: 'password_change',
      }),
    );
  });

  it('paging_keeps_filters', async () => {
    const invoke = renderAuditPage({
      list_audit_logs: (args) => {
        const q = args.query as { page: number };
        return page([entry({ id: `id-${q.page}` })], 120, q.page, 50);
      },
    });
    const user = userEvent.setup();
    await screen.findByText('共 120 筆');

    await user.click(screen.getByLabelText('動作'));
    await user.click(await screen.findByTitle('登入'));
    await user.click(screen.getByRole('button', { name: /查\s*詢/ }));
    await waitFor(() => expect(auditQueries(invoke).at(-1)).toMatchObject({ action: 'login' }));

    await user.click(screen.getByTitle('2'));

    await waitFor(() =>
      expect(auditQueries(invoke).at(-1)).toEqual({ ...EMPTY_QUERY, action: 'login', page: 2 }),
    );
  });

  it('clear_resets_filters', async () => {
    const invoke = renderAuditPage();
    const user = userEvent.setup();
    await screen.findByRole('cell', { name: '2026-11-13 09:00:00' });

    await user.click(screen.getByLabelText('動作'));
    await user.click(await screen.findByTitle('登出'));
    await user.click(screen.getByRole('button', { name: /查\s*詢/ }));
    await waitFor(() => expect(auditQueries(invoke).at(-1)).toMatchObject({ action: 'logout' }));

    await user.click(screen.getByRole('button', { name: /清\s*除/ }));

    await waitFor(() => expect(auditQueries(invoke).at(-1)).toEqual(EMPTY_QUERY));
  });

  it('expanded_row_shows_before_and_after', async () => {
    renderAuditPage();
    const user = userEvent.setup();
    const cell = await screen.findByRole('cell', { name: '2026-11-13 09:00:00' });

    await user.click(within(cell.closest('tr')!).getByRole('button', { name: /展開/ }));

    expect(await screen.findByText('變更前')).toBeInTheDocument();
    expect(screen.getByText('變更後')).toBeInTheDocument();
    expect(screen.getByText(/"display_name": "陳編輯"/)).toBeInTheDocument();
    expect(screen.getByText(/"display_name": "陳大編"/)).toBeInTheDocument();
  });

  it('user_filter_hidden_without_user_manage', async () => {
    const auditor = { ...ADMIN, permissions: ['audit.read'] };
    window.location.hash = '#/audit-logs';
    const invoke = mockCommands({
      ...startupCommands(auditor),
      list_audit_logs: () => page([]),
    });
    renderWithProviders(<App />);

    expect(await screen.findByLabelText('動作')).toBeInTheDocument();
    expect(screen.queryByLabelText('使用者')).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalledWith('list_users', expect.anything());
  });
});
