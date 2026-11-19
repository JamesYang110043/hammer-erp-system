import { screen, waitFor, within } from '@testing-library/react';
import type { UserEvent } from '@testing-library/user-event';
import { expect } from 'vitest';

/** 左側選單（AppLayout 的 `<nav aria-label="主選單">`）。 */
export function sideNav(): HTMLElement {
  return screen.getByRole('navigation', { name: '主選單' });
}

/** 區塊（可收折的子選單）的標題，例如「基本資料」。 */
export function navSectionTitle(label: string): HTMLElement {
  return within(sideNav()).getByRole('menuitem', { name: label });
}

/** 區塊底下的項目名稱（依顯示順序）；區塊收起時為空清單。 */
export function navSectionItems(label: string): string[] {
  const section = navSectionTitle(label).closest('li')!;
  const list = section.querySelector<HTMLElement>(':scope > ul[role="menu"]');
  if (!list || list.classList.contains('ant-menu-hidden')) {
    return [];
  }
  return within(list)
    .getAllByRole('menuitem')
    .map((el) => el.textContent ?? '');
}

/** 展開區塊（已經展開時不動作）。 */
export async function openNavSection(user: UserEvent, label: string) {
  const title = navSectionTitle(label);
  if (title.getAttribute('aria-expanded') !== 'true') {
    await user.click(title);
  }
  await waitFor(() => expect(navSectionTitle(label)).toHaveAttribute('aria-expanded', 'true'));
}
