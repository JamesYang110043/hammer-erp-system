import { describe, expect, it } from 'vitest';

import type { EntityDefinition, Record as DataRecord } from '../bindings';
import { fieldDef } from '../test/schemaFixtures';
import {
  defaultTaxType,
  documentTotals,
  lineAmount,
  lineFormFields,
  orderNotReceivable,
  purchasingEntities,
  rateNote,
  receiptLinesFromOrder,
  sameDecimal,
  trimRate,
  unitCostTwd,
  unreceivedQty,
} from './purchasing';

describe('purchasing amounts', () => {
  it('example_twd_taxable', () => {
    // §8.8 範例一：台幣、應稅（d = 0、稅率 0.05、匯率 1）——與伺服器 amounts.rs 的測試相同
    const a1 = lineAmount('3', '33.3333', 0)!;
    const a2 = lineAmount('2', '150.5000', 0)!;
    expect([a1, a2]).toEqual(['100.0000', '301.0000']);
    expect(documentTotals([a1, a2], '0.0500', 0, '1.000000')).toEqual({
      subtotal: '401.0000',
      tax_amount: '20.0000',
      total: '421.0000',
      total_twd: '421.00',
    });
    expect(unitCostTwd('33.3333', '1')).toBe('33.3333');
  });

  it('example_usd_zero_rated', () => {
    // §8.8 範例二：美元、零稅率（d = 2、匯率 32.150000）
    const a1 = lineAmount('10', '12.3450', 2)!;
    const a2 = lineAmount('3', '7.5000', 2)!;
    expect([a1, a2]).toEqual(['123.4500', '22.5000']);
    expect(documentTotals([a1, a2], '0.0000', 2, '32.150000')).toEqual({
      subtotal: '145.9500',
      tax_amount: '0.0000',
      total: '145.9500',
      total_twd: '4692.29',
    });
    expect(unitCostTwd('12.3450', '32.150000')).toBe('396.8918');
    expect(unitCostTwd('7.5000', '32.150000')).toBe('241.1250');
  });

  it('rounds_midpoint_away_from_zero', () => {
    expect(lineAmount('1', '0.5', 0)).toBe('1.0000');
    expect(lineAmount('1', '2.5', 0)).toBe('3.0000');
    expect(lineAmount('1', '2.4999', 0)).toBe('2.0000');
    expect(lineAmount('3', '0.005', 2)).toBe('0.0200');
    // 稅額四捨五入：101 × 0.05 = 5.05 → 5（d = 0）
    expect(documentTotals(['101.0000'], '0.05', 0, null).tax_amount).toBe('5.0000');
    expect(documentTotals(['110.0000'], '0.05', 0, null).tax_amount).toBe('6.0000');
  });

  it('handles_missing_values', () => {
    expect(lineAmount('', '1', 0)).toBeUndefined();
    expect(lineAmount('abc', '1', 0)).toBeUndefined();
    expect(lineAmount(undefined, '1', 0)).toBeUndefined();
    // 沒有匯率時台幣總計為 null
    expect(documentTotals(['10.0000'], '0', 2, undefined).total_twd).toBeNull();
    expect(documentTotals([], '0.05', 0, '1')).toEqual({
      subtotal: '0.0000',
      tax_amount: '0.0000',
      total: '0.0000',
      total_twd: '0.00',
    });
    expect(unitCostTwd('1', '')).toBeUndefined();
  });

  it('default_tax_type_by_currency', () => {
    expect(defaultTaxType(true)).toBe('taxable');
    expect(defaultTaxType(false)).toBe('zero_rated');
  });

  it('compares_and_trims_rates', () => {
    expect(sameDecimal('32.15', '32.150000')).toBe(true);
    expect(sameDecimal('32.15', '32.16')).toBe(false);
    expect(sameDecimal('', '1')).toBe(false);
    expect(trimRate('32.150000')).toBe('32.15');
    expect(trimRate('1.000000')).toBe('1');
  });
});

describe('rate note', () => {
  const found = {
    currency_code: 'USD',
    date: '2026-11-18',
    rate: '32.150000',
    rate_date: '2026-11-15',
  };

  it('shows_source_date_or_missing', () => {
    expect(rateNote(found, '32.15', false)).toEqual({
      type: 'secondary',
      text: '匯率日期：2026-11-15',
    });
    expect(rateNote(found, '33', false)).toEqual({
      type: 'secondary',
      text: '手動輸入；適用匯率為 2026-11-15 的匯率 32.15',
    });
    expect(rateNote(found, undefined, false)?.type).toBe('warning');
    expect(rateNote({ ...found, rate: null, rate_date: null }, undefined, false)).toEqual({
      type: 'warning',
      text: '找不到 USD 在 2026-11-18 以前的匯率，請輸入匯率',
    });
    expect(rateNote(undefined, '1', true)?.text).toBe('本位幣，匯率固定為 1');
    expect(rateNote(undefined, '1', false)).toBeUndefined();
  });
});

describe('purchase order to goods receipt', () => {
  const line = (id: string, qty: string, received: string): DataRecord =>
    ({
      id,
      title: id,
      record_no: null,
      data: { product_id: `p-${id}`, qty, unit_price: '12.3450', received_qty: received },
      formula_errors: {},
      relations: {},
      attachments: {},
      document: null,
      created_at: '2026-11-18T01:00:00Z',
      created_by: null,
      updated_at: '2026-11-18T01:00:00Z',
      updated_by: null,
      version: 1,
    }) as unknown as DataRecord;

  it('copies_unreceived_lines', () => {
    const order = {
      lines: {
        purchase_order_line: [
          line('l1', '10.0000', '4.0000'),
          line('l2', '5.0000', '5.0000'),
          line('l3', '2.5000', '0.0000'),
        ],
      },
    };
    expect(receiptLinesFromOrder(order)).toEqual([
      {
        purchase_order_line_id: 'l1',
        product_id: 'p-l1',
        qty: '6.0000',
        unit_price: '12.3450',
        lot_no: null,
      },
      {
        purchase_order_line_id: 'l3',
        product_id: 'p-l3',
        qty: '2.5000',
        unit_price: '12.3450',
        lot_no: null,
      },
    ]);
    expect(receiptLinesFromOrder({ lines: {} })).toEqual([]);
    expect(unreceivedQty('3', null)).toBe('3.0000');
  });

  it('checks_order_status', () => {
    expect(orderNotReceivable('confirmed')).toBeUndefined();
    expect(orderNotReceivable('partially_received')).toBeUndefined();
    expect(orderNotReceivable('draft')).toBe('採購單尚未確認');
    expect(orderNotReceivable('closed')).toBe('採購單已結案');
    expect(orderNotReceivable('void')).toBe('採購單已作廢');
  });

  it('carries_hidden_purchase_order_line', () => {
    const fields = [
      fieldDef({ field_key: 'purchase_order_line_id', is_visible: false, sort_order: 10 }),
      fieldDef({ field_key: 'qty', sort_order: 20 }),
      fieldDef({ field_key: 'secret', is_visible: false, sort_order: 30 }),
    ];
    expect(lineFormFields('goods_receipt_line', fields).map((f) => f.field_key)).toEqual([
      'qty',
      'purchase_order_line_id',
    ]);
    expect(lineFormFields('purchase_order_line', fields).map((f) => f.field_key)).toEqual(['qty']);
  });

  it('lists_menu_entities', () => {
    const entity = (key: string, is_active = true) => ({ key, is_active }) as EntityDefinition;
    expect(
      purchasingEntities([
        entity('goods_receipt'),
        entity('purchase_order'),
        entity('purchase_return'),
        entity('x', false),
      ]).map((e) => e.key),
    ).toEqual(['purchase_order', 'goods_receipt']);
  });
});
