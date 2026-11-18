import { describe, expect, it } from 'vitest';

import {
  PRODUCT,
  STOCK_ADJUSTMENT,
  STOCK_ADJUSTMENT_FIELDS,
  STOCK_ADJUSTMENT_LINE,
  lot,
} from '../test/documentFixtures';
import { REPAIR } from '../test/schemaFixtures';
import {
  documentKeyOfLine,
  findLot,
  formatAmount,
  formatQtyChange,
  formatUnitCost,
  inventoryEntities,
  lotOptionLabel,
  lotsIncludeEmpty,
  usesExistingLot,
  withFormDefaults,
} from './inventory';

describe('inventory', () => {
  it('inventory_menu_lists_active_stock_adjustment', () => {
    expect(inventoryEntities([REPAIR, PRODUCT, STOCK_ADJUSTMENT_LINE, STOCK_ADJUSTMENT])).toEqual([
      STOCK_ADJUSTMENT,
    ]);
    expect(inventoryEntities([{ ...STOCK_ADJUSTMENT, is_active: false }])).toEqual([]);
  });

  it('line_type_decides_lot_input', () => {
    expect(usesExistingLot('gain_new')).toBe(false);
    expect(usesExistingLot('gain_existing')).toBe(true);
    expect(usesExistingLot('loss')).toBe(true);
    expect(usesExistingLot(undefined)).toBe(false);
    // 盤盈加回可以選已用完的批號；盤虧只能扣有數量的批號
    expect(lotsIncludeEmpty('gain_existing')).toBe(true);
    expect(lotsIncludeEmpty('loss')).toBe(false);
  });

  it('lot_options_and_lookup', () => {
    expect(lotOptionLabel(lot())).toBe('L-20261101（剩 35.5，2026-11-01 入庫）');
    const lots = [lot(), lot({ id: 'l2', lot_no: 'B-02' })];
    expect(findLot(lots, ' b-02 ')?.id).toBe('l2');
    expect(findLot(lots, '')).toBeUndefined();
    expect(findLot(lots, null)).toBeUndefined();
    expect(findLot(undefined, 'B-02')).toBeUndefined();
  });

  it('adjustment_date_defaults_to_today', () => {
    const fields = withFormDefaults('stock_adjustment', STOCK_ADJUSTMENT_FIELDS, '2026-11-18');
    expect(fields.find((f) => f.field_key === 'adjustment_date')?.default_value).toBe('2026-11-18');
    // 其他欄位與其他資料類別不變
    expect(fields.find((f) => f.field_key === 'adjustment_type')?.default_value).toBe('count');
    expect(withFormDefaults('repair', STOCK_ADJUSTMENT_FIELDS, '2026-11-18')).toBe(
      STOCK_ADJUSTMENT_FIELDS,
    );
  });

  it('formats_numbers', () => {
    expect(formatQtyChange('5.5000')).toBe('+5.5');
    expect(formatQtyChange('-1200.0000')).toBe('-1,200');
    expect(formatUnitCost('12.5000')).toBe('12.50');
    expect(formatUnitCost('1234.1250')).toBe('1,234.125');
    expect(formatUnitCost('0.0000')).toBe('0.00');
    expect(formatAmount('3450.00')).toBe('3,450.00');
  });

  it('maps_line_key_to_document', () => {
    const entities = [STOCK_ADJUSTMENT, STOCK_ADJUSTMENT_LINE];
    expect(documentKeyOfLine(entities, 'stock_adjustment_line')).toBe('stock_adjustment');
    expect(documentKeyOfLine(entities, 'goods_receipt_line')).toBeUndefined();
    expect(documentKeyOfLine(undefined, 'stock_adjustment_line')).toBeUndefined();
  });
});
