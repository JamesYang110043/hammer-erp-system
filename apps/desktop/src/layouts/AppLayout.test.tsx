import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { App } from '../App';
import type { CurrentUser, EntityDefinition } from '../bindings';
import { PERMISSIONS } from '../bindings';
import { ADMIN, startupCommands } from '../test/fixtures';
import { mockCommands } from '../test/ipc';
import { navSectionItems, navSectionTitle, openNavSection, sideNav } from '../test/nav';
import { renderWithProviders } from '../test/render';
import { REPAIR } from '../test/schemaFixtures';
import { NAV_OPEN_KEYS_STORAGE_KEY } from './navSections';

const PAGE_LOAD_TIMEOUT = 5000;

function systemEntity(key: string, label: string, kind: EntityDefinition['kind']) {
  return {
    ...REPAIR,
    id: `entity-${key}`,
    key,
    label,
    kind,
    icon: null,
    is_system: true,
    storage: 'table',
  } satisfies EntityDefinition;
}

const CUSTOMER = systemEntity('customer', '客戶', 'master');
const PURCHASE_ORDER = systemEntity('purchase_order', '採購單', 'document');

/** 從 `hash` 開啟主畫面，等左側選單載入資料類別。 */
async function renderAt(hash: string, user: CurrentUser = ADMIN) {
  window.location.hash = hash;
  mockCommands({
    ...startupCommands(user),
    list_entities: () => [CUSTOMER, PURCHASE_ORDER, REPAIR],
  });
  const view = renderWithProviders(<App />);
  await screen.findByRole('navigation', { name: '主選單' }, { timeout: PAGE_LOAD_TIMEOUT });
  return view;
}

const isOpen = (label: string) => navSectionTitle(label).getAttribute('aria-expanded') === 'true';

describe('左側選單區塊收折', () => {
  it('direct_entry_opens_the_current_section_and_selects_the_item', async () => {
    await renderAt('#/entities/customer');

    await waitFor(() => expect(isOpen('基本資料')).toBe(true));
    expect(within(sideNav()).getByRole('menuitem', { name: /客戶/ })).toHaveClass(
      'ant-menu-item-selected',
    );
    // 其他區塊第一次使用時是收起的
    expect(isOpen('進貨')).toBe(false);
    expect(navSectionItems('進貨')).toEqual([]);
  });

  it('section_collapses_and_expands', async () => {
    await renderAt('#/entities/customer');
    const user = userEvent.setup();
    await waitFor(() => expect(isOpen('基本資料')).toBe(true));

    await user.click(navSectionTitle('基本資料'));
    await waitFor(() => expect(isOpen('基本資料')).toBe(false));
    // 收起後只剩區塊標題（收合動畫結束後隱藏）
    await waitFor(() => expect(navSectionItems('基本資料')).toEqual([]));
    expect(navSectionTitle('基本資料')).toBeVisible();

    await openNavSection(user, '基本資料');
    expect(navSectionItems('基本資料')).toContain('客戶');
  });

  it('navigating_opens_the_section_of_the_new_page', async () => {
    await renderAt('#/entities/customer');
    await waitFor(() => expect(isOpen('基本資料')).toBe(true));

    window.location.hash = '#/entities/purchase_order';

    await waitFor(() => expect(isOpen('進貨')).toBe(true));
    expect(within(sideNav()).getByRole('menuitem', { name: /採購單/ })).toHaveClass(
      'ant-menu-item-selected',
    );
    // 原本展開的區塊維持展開
    expect(isOpen('基本資料')).toBe(true);
  });

  it('navigating_within_a_collapsed_section_opens_it_again', async () => {
    await renderAt('#/entities/customer');
    const user = userEvent.setup();
    await waitFor(() => expect(isOpen('基本資料')).toBe(true));
    await user.click(navSectionTitle('基本資料'));
    await waitFor(() => expect(isOpen('基本資料')).toBe(false));

    // 同一區塊的另一頁
    window.location.hash = '#/exchange-rates';

    await waitFor(() => expect(isOpen('基本資料')).toBe(true));
  });

  it('open_state_survives_reopening', async () => {
    const first = await renderAt('#/connection');
    const user = userEvent.setup();
    await screen.findByRole('menuitem', { name: '進貨' });
    await openNavSection(user, '進貨');
    await openNavSection(user, '主檔');
    await user.click(navSectionTitle('主檔'));
    await waitFor(() => expect(isOpen('主檔')).toBe(false));
    expect(JSON.parse(window.localStorage.getItem(NAV_OPEN_KEYS_STORAGE_KEY)!)).toEqual([
      'purchasing-group',
    ]);

    // 重新開啟程式
    first.unmount();
    await renderAt('#/connection');
    await screen.findByRole('menuitem', { name: '進貨' });
    expect(isOpen('進貨')).toBe(true);
    expect(navSectionItems('進貨')).toEqual(['採購單']);
    expect(isOpen('主檔')).toBe(false);
    expect(isOpen('基本資料')).toBe(false);
  });

  it('sections_without_permission_are_hidden', async () => {
    // 沒有 record.read：看不到資料類別與匯率，基本資料、進貨區塊都不顯示
    await renderAt('#/connection', { ...ADMIN, permissions: [PERMISSIONS.USER_MANAGE] });

    expect(within(sideNav()).getByRole('menuitem', { name: /帳號管理/ })).toBeInTheDocument();
    expect(within(sideNav()).queryByRole('menuitem', { name: '基本資料' })).not.toBeInTheDocument();
    expect(within(sideNav()).queryByRole('menuitem', { name: '進貨' })).not.toBeInTheDocument();
  });
});
