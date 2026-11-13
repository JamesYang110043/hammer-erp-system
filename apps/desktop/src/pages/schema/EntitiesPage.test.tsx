import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../../App';
import { KEY_MESSAGES } from '../../schema/keyRules';
import { ADMIN, VIEWER, startupCommands } from '../../test/fixtures';
import { commandError, mockCommands } from '../../test/ipc';
import type { CommandHandler } from '../../test/ipc';
import { renderWithProviders } from '../../test/render';
import { REPAIR, entityDetail } from '../../test/schemaFixtures';

const PAGE_LOAD_TIMEOUT = 5000;

function renderEntitiesPage(extra: Record<string, CommandHandler> = {}) {
  window.location.hash = '#/schema';
  const invoke = mockCommands({
    ...startupCommands(ADMIN),
    list_entities: () => [REPAIR],
    ...extra,
  });
  renderWithProviders(<App />);
  return invoke;
}

async function openCreateDialog() {
  const user = userEvent.setup();
  await screen.findByText('repair', { selector: 'td code' }, { timeout: PAGE_LOAD_TIMEOUT });
  await user.click(screen.getByRole('button', { name: /新增資料類別/ }));
  const dialog = within(await screen.findByRole('dialog'));
  return { user, dialog };
}

describe('資料類別頁', () => {
  it('lists_entities', async () => {
    renderEntitiesPage();
    const cell = await screen.findByText(
      'repair',
      { selector: 'td code' },
      { timeout: PAGE_LOAD_TIMEOUT },
    );
    const row = within(cell.closest('tr')!);
    expect(row.getByRole('button', { name: /維修單/ })).toBeInTheDocument();
    expect(row.getByText('主檔')).toBeInTheDocument();
    expect(row.getByText('自訂')).toBeInTheDocument();
  });

  it('creates_entity_and_opens_fields_page', async () => {
    const created = { ...REPAIR, key: 'repair_order', label: '維修工單' };
    const invoke = renderEntitiesPage({
      create_entity: () => ({ entity: created, fields: [] }),
      get_entity: () => ({ ...entityDetail([]), entity: created }),
    });
    const { user, dialog } = await openCreateDialog();

    await user.type(dialog.getByLabelText('代碼'), 'repair_order');
    await user.type(dialog.getByLabelText('名稱'), ' 維修工單 ');
    await user.click(dialog.getByRole('button', { name: /建\s*立/ }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('create_entity', {
        req: {
          key: 'repair_order',
          label: '維修工單',
          icon: null,
          kind: 'master',
          title_template: null,
          sort_order: 0,
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
        commandError({
          kind: 'api',
          code: 'VALIDATION',
          message: '資料類別代碼已存在',
          details: { field: 'key' },
        }),
    });
    const { user, dialog } = await openCreateDialog();
    await user.type(dialog.getByLabelText('代碼'), 'repair');
    await user.type(dialog.getByLabelText('名稱'), '維修單');
    await user.click(dialog.getByRole('button', { name: /建\s*立/ }));
    expect(await dialog.findByText('資料類別代碼已存在')).toBeInTheDocument();
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
