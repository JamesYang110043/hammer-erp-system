import { describe, expect, it } from 'vitest';

import type { EntityDefinition, Record as DataRecord } from '../bindings';
import {
  availableAllowance,
  convertedUnitPrice,
  crossPeriodWarning,
  invoiceLineAmount,
  invoiceTotals,
  invoicingActionLabel,
  invoicingEntities,
  invoicingLineKey,
  isInvoicingDocument,
  linesFromSource,
  postedLabel,
  productDescription,
  sourceDescription,
  taxPeriod,
  taxPeriodLabel,
} from './invoicing';

describe('期別（§8.11「期別」）', () => {
  it('每兩個月一期，取該期第一個月', () => {
    expect(taxPeriod('2026-11-19')).toBe('202611');
    expect(taxPeriod('2026-12-05')).toBe('202611');
    expect(taxPeriod('2026-02-28')).toBe('202601');
    expect(taxPeriod('2026-13-01')).toBeUndefined();
    expect(taxPeriod('2026/11/19')).toBeUndefined();
    expect(taxPeriod(null)).toBeUndefined();
  });

  it('顯示文字', () => {
    expect(taxPeriodLabel('202611')).toBe('2026 年 11–12 月');
    expect(taxPeriodLabel('202601')).toBe('2026 年 1–2 月');
    expect(taxPeriodLabel('')).toBeUndefined();
  });

  it('跨期作廢的警告', () => {
    expect(crossPeriodWarning('2026-11-20', '202611')).toBeUndefined();
    expect(crossPeriodWarning('2026-11-20', '202609')).toBe(
      '作廢日期與發票不同期（發票 2026 年 9–10 月），請確認是否需要另外處理申報',
    );
    expect(crossPeriodWarning('', '202609')).toBeUndefined();
  });
});

describe('金額與手動稅額（§8.11 範例）', () => {
  it('範例一：三聯式、應稅', () => {
    const amounts = [invoiceLineAmount('3', '100', 0)!, invoiceLineAmount('2', '333.3333', 0)!];
    expect(amounts).toEqual(['300.0000', '667.0000']);
    expect(invoiceTotals(amounts, '0.05', false, 0, null)).toEqual({
      subtotal: '967.0000',
      tax_amount: '48.0000',
      total: '1015.0000',
    });
    expect(invoiceTotals(amounts, '0.05', false, 0, '49')).toEqual({
      subtotal: '967.0000',
      tax_amount: '49.0000',
      total: '1016.0000',
    });
    // 相差超過 1 元：不套用（存檔時伺服器回報）
    expect(invoiceTotals(amounts, '0.05', false, 0, '50').tax_amount).toBe('48.0000');
  });

  it('範例二：二聯式、應稅（總計不變）', () => {
    const amounts = [invoiceLineAmount('4', '200', 0)!, invoiceLineAmount('1', '200', 0)!];
    expect(invoiceTotals(amounts, '0.05', true, 0, '')).toEqual({
      subtotal: '952.0000',
      tax_amount: '48.0000',
      total: '1000.0000',
    });
    expect(invoiceTotals(amounts, '0.05', true, 0, '47')).toEqual({
      subtotal: '953.0000',
      tax_amount: '47.0000',
      total: '1000.0000',
    });
  });

  it('零稅率不套用手動稅額', () => {
    expect(invoiceTotals(['500.0000'], '0', false, 0, '1').tax_amount).toBe('0.0000');
  });
});

describe('出貨明細帶入發票明細', () => {
  it('單價扣掉折扣，依含稅基準換算', () => {
    expect(convertedUnitPrice('120', '0.15', false, false, '0.05')).toBe('102.0000');
    expect(convertedUnitPrice('105', '0', true, false, '0.05')).toBe('100.0000');
    expect(convertedUnitPrice('100', '0', false, true, '0.05')).toBe('105.0000');
    expect(convertedUnitPrice('33.3333', '0', true, false, '0.05')).toBe('31.7460');
    expect(convertedUnitPrice('', '0', false, false, '0.05')).toBeUndefined();
  });

  it('品名取商品標題去掉料號', () => {
    expect(productDescription('A001 不鏽鋼螺絲 M6')).toBe('不鏽鋼螺絲 M6');
    expect(productDescription('安裝費')).toBe('安裝費');
    expect(productDescription(undefined)).toBe('');
  });

  it('source description prefers the document product name (M11)', () => {
    const line = (name: unknown) =>
      ({
        data: { product_name: name },
        relations: {
          product_id: { id: 'p1', title: 'A001 扳手 10mm', deleted: false, inactive: false },
        },
      }) as never;
    expect(sourceDescription(line('X 型扳手'))).toBe('X 型扳手');
    expect(sourceDescription(line(undefined))).toBe('扳手 10mm');
    expect(sourceDescription(line(' '))).toBe('扳手 10mm');
  });

  it('linesFromSource', () => {
    const shipment = {
      data: { tax_included: false },
      lines: {
        shipment_line: [
          {
            data: {
              product_id: 'p1',
              qty: '10.0000',
              unit_price: '120.0000',
              discount_rate: '0.1500',
            },
            relations: {
              product_id: {
                id: 'p1',
                title: 'A001 不鏽鋼螺絲 M6',
                deleted: false,
                inactive: false,
              },
            },
          },
        ],
      },
    } as unknown as Pick<DataRecord, 'data' | 'lines'>;
    expect(linesFromSource(shipment, 'shipment_line', true, '0.05')).toEqual([
      { product_id: 'p1', description: '不鏽鋼螺絲 M6', qty: '10.0000', unit_price: '107.1000' },
    ]);
  });
});

describe('其他', () => {
  it('可折讓金額', () => {
    expect(availableAllowance('2100.0000', ['1050.0000'])).toBe('1050.0000');
    expect(availableAllowance('2100.0000', [])).toBe('2100.0000');
    expect(availableAllowance('100', ['150'])).toBe('0.0000');
    expect(availableAllowance(null, [])).toBeUndefined();
  });

  it('單據、選單、名稱', () => {
    expect(isInvoicingDocument('sales_invoice')).toBe(true);
    expect(isInvoicingDocument('shipment')).toBe(false);
    expect(invoicingLineKey('sales_invoice')).toBe('sales_invoice_line');
    expect(invoicingLineKey('sales_allowance')).toBe('sales_allowance_line');
    expect(invoicingLineKey('purchase_invoice')).toBeUndefined();
    expect(postedLabel('sales_invoice')).toBe('已開立');
    expect(postedLabel('purchase_invoice')).toBe('已登錄');
    expect(postedLabel('shipment')).toBeUndefined();
    expect(invoicingActionLabel('sales_invoice', 'post')).toBe('開立');
    expect(invoicingActionLabel('purchase_invoice', 'post')).toBe('登錄');
    expect(invoicingActionLabel('sales_invoice', 'void')).toBeUndefined();
    const entity = (key: string, patch: Partial<EntityDefinition> = {}) =>
      ({ key, is_active: true, is_system: true, ...patch }) as EntityDefinition;
    expect(
      invoicingEntities([
        entity('purchase_allowance'),
        entity('sales_invoice'),
        entity('sales_allowance', { is_system: false }),
        entity('shipment'),
      ]).map((e) => e.key),
    ).toEqual(['sales_invoice', 'purchase_allowance']);
  });
});

describe('顯示', () => {
  it('台幣金額、狀態說明', async () => {
    const { invoicingStatusNote, moneyText, sumAmounts } = await import('./invoicing');
    expect(moneyText('1070.0000')).toBe('1,070');
    expect(moneyText('1234567.5')).toBe('1,234,568');
    expect(sumAmounts(['100.0000', '5', null])).toBe('105.0000');
    expect(invoicingStatusNote('sales_invoice', '此單據已過帳，不能修改。過帳時間：x')).toBe(
      '此單據已開立，不能修改。開立時間：x',
    );
    expect(invoicingStatusNote('purchase_invoice', '此單據已過帳，不能修改。過帳時間：x')).toBe(
      '此單據已登錄，不能修改。登錄時間：x',
    );
    expect(invoicingStatusNote('shipment', '此單據已過帳')).toBe('此單據已過帳');
  });
});

describe('進項憑證、進貨折讓', () => {
  it('總計 = 未稅 + 稅額', async () => {
    const { enteredTotal, hasEnteredAmounts } = await import('./invoicing');
    expect(enteredTotal('200', '10')).toBe('210.0000');
    expect(enteredTotal('', '10')).toBeUndefined();
    expect(hasEnteredAmounts('purchase_invoice')).toBe(true);
    expect(hasEnteredAmounts('purchase_allowance')).toBe(true);
    expect(hasEnteredAmounts('sales_invoice')).toBe(false);
  });

  it('依台幣進貨單建議金額，外幣不建議', async () => {
    const { suggestedInvoiceAmounts } = await import('./invoicing');
    expect(
      suggestedInvoiceAmounts(
        [
          { currency_code: 'TWD', total_twd: '210.00' },
          { currency_code: 'TWD', total_twd: '1050.00' },
        ],
        '0.05',
      ),
    ).toEqual({ subtotal: '1200', tax_amount: '60' });
    // 1000 ÷ 1.05 = 952.38 → 952、稅 48
    expect(
      suggestedInvoiceAmounts([{ currency_code: 'TWD', total_twd: '1000.00' }], '0.05'),
    ).toEqual({ subtotal: '952', tax_amount: '48' });
    expect(
      suggestedInvoiceAmounts([{ currency_code: 'USD', total_twd: '3215.00' }], '0'),
    ).toBeUndefined();
    expect(suggestedInvoiceAmounts([], '0.05')).toBeUndefined();
  });
});
