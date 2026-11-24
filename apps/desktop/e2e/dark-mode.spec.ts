import { expect, test } from '@playwright/test';
import type { Page } from '@playwright/test';

import type { Record as DataRecord } from '../src/bindings';
import {
  ADJUSTMENT_ID,
  STOCK_ADJUSTMENT,
  STOCK_ADJUSTMENT_FIELDS,
  adjustment,
} from '../src/test/documentFixtures';
import { mockTauri } from './tauri-mock';

const ADMIN = {
  id: '0196f5a0-0000-7000-8000-000000000001',
  username: 'admin',
  display_name: '王管理',
  role: 'admin',
  permissions: ['record.post', 'record.read', 'record.write', 'schema.manage'],
  must_change_password: false,
};

async function mockServer(page: Page) {
  const record: DataRecord = adjustment('posted');
  await mockTauri(page, {
    get_server_url: () => 'http://localhost:8080',
    health_check: () => ({ status: 'ok', version: '0.1.0', database: 'ok' }),
    setup_status: () => ({ needs_setup: false }),
    current_user: () => ADMIN,
    list_entities: () => [STOCK_ADJUSTMENT],
    get_entity: () => ({ entity: STOCK_ADJUSTMENT, fields: STOCK_ADJUSTMENT_FIELDS }),
    get_default_layout: () => ({
      source: 'generated',
      layout: null,
      config: {
        columns: ['adjustment_date', 'adjustment_type', 'status', 'notes'].map((k) => ({
          field_key: k,
          width: null,
        })),
        sort: null,
        filters: [],
      },
    }),
    list_layouts: () => [],
    list_records: () => ({ items: [record], total: 1, page: 1, page_size: 50 }),
    get_record: () => record,
  });
}

const theme = (page: Page) => page.evaluate(() => document.documentElement.dataset.theme);

/** 在頁面中量測文字與實際背景（往上找第一個不透明背景）的對比。 */
async function contrastOf(page: Page, selector: string) {
  return page
    .locator(selector)
    .first()
    .evaluate((el) => {
      const parse = (c: string) => {
        const m = /rgba?\(([^)]+)\)/.exec(c)!;
        const [r, g, b, a = 1] = m[1]!.split(',').map((v) => Number.parseFloat(v));
        return { r: r!, g: g!, b: b!, a };
      };
      let bgEl: Element | null = el;
      let bg = parse(getComputedStyle(el).backgroundColor);
      while (bgEl && bg.a === 0) {
        bgEl = bgEl.parentElement;
        bg = bgEl
          ? parse(getComputedStyle(bgEl).backgroundColor)
          : { r: 255, g: 255, b: 255, a: 1 };
      }
      const fg = parse(getComputedStyle(el).color);
      // 半透明文字混合到背景上
      const mix = (f: number, b: number) => f * fg.a + b * (1 - fg.a);
      const lum = ({ r, g, b }: { r: number; g: number; b: number }) => {
        const ch = (v: number) => {
          const c = v / 255;
          return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
        };
        return 0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b);
      };
      const l1 = lum({ r: mix(fg.r, bg.r), g: mix(fg.g, bg.g), b: mix(fg.b, bg.b) });
      const l2 = lum(bg);
      return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05);
    });
}

test('appearance menu switches light, dark and system, and survives a restart', async ({
  page,
}) => {
  await mockServer(page);
  await page.emulateMedia({ colorScheme: 'light' });
  await page.goto('/#/entities/stock_adjustment');
  await expect(page.getByRole('button', { name: /王管理/ })).toBeVisible();
  // 預設跟隨系統（淺色）
  expect(await theme(page)).toBe('light');

  const choose = async (label: string) => {
    await page.getByRole('button', { name: /王管理/ }).click();
    await page.getByRole('menuitem', { name: /外觀/ }).hover();
    await page.getByRole('menuitem', { name: label }).click();
  };

  // 深色：即時生效
  await choose('深色');
  await expect.poll(() => theme(page)).toBe('dark');
  await expect(page.locator('.ant-layout-header')).toHaveCSS('background-color', 'rgb(22, 28, 36)');

  // 重新開啟後維持
  await page.reload();
  await expect(page.getByRole('button', { name: /王管理/ })).toBeVisible();
  expect(await theme(page)).toBe('dark');

  // 淺色
  await choose('淺色');
  await expect.poll(() => theme(page)).toBe('light');

  // 跟隨系統：系統切換時即時跟著變
  await choose('跟隨系統');
  await expect.poll(() => theme(page)).toBe('light');
  await page.emulateMedia({ colorScheme: 'dark' });
  await expect.poll(() => theme(page)).toBe('dark');
  await page.emulateMedia({ colorScheme: 'light' });
  await expect.poll(() => theme(page)).toBe('light');
});

test('dark mode: list and document form text is readable', async ({ page }) => {
  await mockServer(page);
  await page.emulateMedia({ colorScheme: 'dark' });

  await page.goto('/#/entities/stock_adjustment');
  await expect(page.getByRole('row').nth(1)).toBeVisible();
  expect(await theme(page)).toBe('dark');
  for (const selector of [
    '.page-header-title',
    '.ant-table-thead th',
    '.ant-table-tbody td',
    '.ant-breadcrumb-link',
    '.ant-tag[data-tone]',
    '.ant-layout-footer .ant-badge-status-text',
  ]) {
    expect(await contrastOf(page, selector), selector).toBeGreaterThanOrEqual(4.5);
  }

  await page.goto(`/#/entities/stock_adjustment/${ADJUSTMENT_ID}`);
  await expect(page.getByRole('textbox', { name: '單號' })).toBeVisible();
  for (const selector of [
    '.page-header-title',
    '.ant-form-item-label > label',
    '.ant-card .ant-typography-secondary',
    '.page-header .ant-tag',
  ]) {
    expect(await contrastOf(page, selector), selector).toBeGreaterThanOrEqual(4.5);
  }
});
