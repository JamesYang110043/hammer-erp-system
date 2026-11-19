import { expect } from '@playwright/test';
import type { Page } from '@playwright/test';

/** 展開左側選單的區塊（例如「進貨」）；已經展開時不動作。 */
export async function openNavSection(page: Page, label: string) {
  const title = page
    .getByRole('navigation', { name: '主選單' })
    .getByRole('menuitem', { name: label, exact: true });
  if ((await title.getAttribute('aria-expanded')) !== 'true') {
    await title.click();
  }
  await expect(title).toHaveAttribute('aria-expanded', 'true');
}
