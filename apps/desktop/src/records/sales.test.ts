import { describe, expect, it } from 'vitest';

import type { InventoryTransaction_Serialize as InventoryTransaction } from '../bindings';
import { div, format, parse } from './decimal';
import {
  lotPicksError,
  manualLotShortages,
  namedSalesLine,
  normalizeLotNo,
  orderNotShippable,
  overShippedRows,
  previewInputs,
  previewRowErrors,
  lineProductNames,
  productNameOf,
  returnLinesFromShipment,
  rowAllocations,
  salePrice,
  salesEntities,
  salesLineAmount,
  salesReturnableQty,
  salesTotals,
  shipmentLinesFromOrder,
  shipmentNotReturnable,
  splitByLots,
  usedLots,
} from './sales';

describe('sales amounts (docs/architecture.md §8.9 examples)', () => {
  it('example 1: tax excluded, taxable', () => {
    const a1 = salesLineAmount('3', '33.3333', '0', 0);
    const a2 = salesLineAmount('10', '120.0000', '0.15', 0);
    expect(a1).toBe('100.0000');
    expect(a2).toBe('1020.0000');
    expect(salesTotals([a1!, a2!], '0.0500', false, 0)).toEqual({
      subtotal: '1120.0000',
      tax_amount: '56.0000',
      total: '1176.0000',
    });
  });

  it('example 2: tax included, taxable', () => {
    const a1 = salesLineAmount('2', '105.0000', '0', 0);
    const a2 = salesLineAmount('5', '99.0000', '0.1', 0);
    expect(a1).toBe('210.0000');
    expect(a2).toBe('446.0000');
    expect(salesTotals([a1!, a2!], '0.0500', true, 0)).toEqual({
      subtotal: '625.0000',
      tax_amount: '31.0000',
      total: '656.0000',
    });
  });

  it('example 3: tax included, exempt', () => {
    expect(salesTotals(['200.0000', '300.0000'], '0.0000', true, 0)).toEqual({
      subtotal: '500.0000',
      tax_amount: '0.0000',
      total: '500.0000',
    });
  });

  it('empty discount counts as 0; invalid qty gives undefined', () => {
    expect(salesLineAmount('2', '10', '', 0)).toBe('20.0000');
    expect(salesLineAmount('', '10', '0', 0)).toBeUndefined();
  });

  it('sale price: tax excluded as is, tax included adds tax rounded to 4 places', () => {
    expect(salePrice('100', false, '0.05')).toBe('100.0000');
    expect(salePrice('99.9999', true, '0.0500')).toBe('104.9999');
    expect(salePrice('33.3333', true, '0.0500')).toBe('35.0000');
    expect(salePrice(null, true, '0.05')).toBeUndefined();
  });
});

describe('decimal division', () => {
  it('rounds half away from zero', () => {
    expect(format(div(parse('656')!, parse('1.05')!, 0)!, 4)).toBe('625.0000');
    expect(format(div(parse('1')!, parse('8')!, 2)!, 2)).toBe('0.13');
    expect(format(div(parse('-1')!, parse('8')!, 2)!, 2)).toBe('-0.13');
    expect(div(parse('1')!, parse('0')!, 2)).toBeUndefined();
  });
});

describe('sales order to shipment', () => {
  it('lists the sales group in menu order', () => {
    const e = (key: string, is_system = true) => ({ key, is_active: true, is_system }) as never;
    expect(
      salesEntities([
        e('sales_return'),
        e('shipment'),
        e('sales_order'),
        e('sales_order', false),
      ]).map((x: { key: string }) => x.key),
    ).toEqual(['sales_order', 'shipment', 'sales_return']);
  });

  it('only confirmed or partially shipped orders can ship', () => {
    expect(orderNotShippable('confirmed')).toBeUndefined();
    expect(orderNotShippable('partially_shipped')).toBeUndefined();
    expect(orderNotShippable('draft')).toBe('訂單尚未確認');
    expect(orderNotShippable('closed')).toBe('訂單已結案');
    expect(orderNotShippable('void')).toBe('訂單已作廢');
  });

  it('carries unshipped lines with price and discount, lot left empty', () => {
    const order = {
      lines: {
        sales_order_line: [
          {
            id: 'l1',
            data: {
              product_id: 'p1',
              product_name: 'X 型扳手',
              qty: '10.0000',
              shipped_qty: '4.0000',
              unit_price: '120.0000',
              discount_rate: '0.1500',
            },
          },
          { id: 'l2', data: { product_id: 'p2', qty: '3.0000', shipped_qty: '3.0000' } },
        ],
      },
    } as never;
    expect(shipmentLinesFromOrder(order)).toEqual([
      {
        sales_order_line_id: 'l1',
        product_id: 'p1',
        // 訂單明細的品名（§8.9「明細的品名」）
        product_name: 'X 型扳手',
        qty: '6.0000',
        unit_price: '120.0000',
        discount_rate: '0.1500',
        lot_no: null,
      },
    ]);
  });

  it('flags the first row when rows of one order line exceed the unshipped qty', () => {
    const unshipped = new Map([
      ['l1', '6.0000'],
      ['l2', '3.0000'],
    ]);
    const rows = [
      { salesOrderLineId: 'l1', qty: '4' },
      { salesOrderLineId: 'l2', qty: '3' },
      { salesOrderLineId: 'l1', qty: '3' },
      { salesOrderLineId: undefined, qty: '100' },
    ];
    expect([...overShippedRows(rows, unshipped)]).toEqual([[0, '6.0000']]);
    expect(overShippedRows(rows.slice(1), unshipped).size).toBe(0);
  });
});

describe('shipment lots', () => {
  it('normalizes lot numbers', () => {
    expect(normalizeLotNo('  L-01 ')).toBe('L-01');
    expect(normalizeLotNo('  ')).toBeNull();
    expect(normalizeLotNo(undefined)).toBeNull();
  });

  it('sends rows with product and positive qty, maps results back to rows', () => {
    const inputs = previewInputs([
      { productId: 'p1', qty: '12', lotNo: '' },
      { productId: undefined, qty: '1' },
      { productId: 'p1', qty: '0' },
      { productId: 'p1', qty: '3', lotNo: ' L-01 ' },
    ]);
    expect(inputs).toEqual([
      { row: 0, product_id: 'p1', qty: '12', lot_no: null },
      { row: 3, product_id: 'p1', qty: '3', lot_no: 'L-01' },
    ]);
    const alloc = (lot_no: string, qty: string) => ({
      lot_id: lot_no,
      lot_no,
      received_date: '2026-10-02',
      qty,
      unit_cost_twd: '80.0000',
    });
    const result = rowAllocations(inputs, {
      lines: [
        {
          index: 0,
          product_id: 'p1',
          product_type: 'goods',
          qty: '12.0000',
          allocations: [alloc('L-02', '7.0000')],
          shortage: '5.0000',
        },
        {
          index: 1,
          product_id: 'p1',
          product_type: 'goods',
          qty: '3.0000',
          allocations: [alloc('L-01', '1.0000')],
          shortage: '2.0000',
        },
      ],
    });
    expect(result.get(0)).toMatchObject({ manual: false, shortage: '5.0000' });
    expect(result.get(3)).toMatchObject({ manual: true, shortage: '2.0000' });
    expect([...manualLotShortages(result, new Map([[3, 'L-01']]))]).toEqual([
      [3, '批號 L-01 的可用數量不足，還差 2'],
    ]);
    expect(
      previewRowErrors(inputs, {
        lines: [{ detail_key: null, index: 1, field_key: 'lot_no', message: '找不到此批號' }],
      }),
    ).toEqual([{ row: 3, field_key: 'lot_no', message: '找不到此批號' }]);
  });

  it('lot picks must add up to the line qty and stay within remaining qty', () => {
    const remaining = new Map([
      ['l-01', '5.0000'],
      ['l-02', '7.0000'],
    ]);
    expect(
      lotPicksError(
        [
          { lot_no: 'L-01', qty: '5' },
          { lot_no: 'L-02', qty: '7' },
        ],
        '12.0000',
        remaining,
      ),
    ).toBeUndefined();
    expect(
      lotPicksError(
        [
          { lot_no: 'L-01', qty: '5' },
          { lot_no: 'L-02', qty: '6' },
        ],
        '12',
        remaining,
      ),
    ).toBe('批號數量合計 11，與出貨數量 12 不符');
    expect(lotPicksError([{ lot_no: 'L-01', qty: '6' }], '6', remaining)).toBe('批號 L-01 只剩 5');
    expect(lotPicksError([{ lot_no: 'L-01', qty: '' }], '6', remaining)).toBe(
      '請至少輸入一個批號的數量',
    );
  });

  it('splits a row into one row per lot, keeping the order line', () => {
    const row = {
      __id: 'line-1',
      sales_order_line_id: 'l1',
      product_id: 'p1',
      qty: '12',
      unit_price: '100',
      lot_no: null,
    };
    expect(
      splitByLots(row, [
        { lot_no: 'L-01', qty: '5' },
        { lot_no: 'L-03', qty: '' },
        { lot_no: 'L-02', qty: '7' },
      ]),
    ).toEqual([
      { ...row, qty: '5', lot_no: 'L-01' },
      { sales_order_line_id: 'l1', product_id: 'p1', qty: '7', unit_price: '100', lot_no: 'L-02' },
    ]);
  });

  it('lists the lots used by a posted shipment in line and deduction order', () => {
    const txn = (id: string, line: string, lot: string, qty: string, cost: string) =>
      ({
        id,
        txn_type: 'shipment',
        product_id: 'p1',
        product_code: 'A001',
        product_name: '螺絲',
        lot_no: lot,
        qty_change: qty,
        unit_cost_twd: cost,
        ref_line_id: line,
      }) as InventoryTransaction;
    // 異動由新到舊：第 2 列（L-03）最後扣，第 1 列先扣 L-01 再扣 L-02
    const used = usedLots(
      [
        txn('t3', 'b', 'L-03', '-1.0000', '90.0000'),
        txn('t2', 'a', 'L-02', '-7.0000', '82.5000'),
        txn('t1', 'a', 'L-01', '-5.0000', '80.0000'),
      ],
      ['a', 'b'],
    );
    expect(used.map((u) => [u.row, u.lotNo, u.qty, u.costTwd])).toEqual([
      [0, 'L-01', '5.0000', '400.00'],
      [0, 'L-02', '7.0000', '577.50'],
      [1, 'L-03', '1.0000', '90.00'],
    ]);
  });
});

describe('sales return', () => {
  const shipment = {
    lines: {
      shipment_line: [
        {
          id: 's1',
          data: {
            product_id: 'p1',
            qty: '12.0000',
            unit_price: '120.0000',
            discount_rate: '0.1500',
            lot_no: null,
          },
        },
        { id: 's2', data: { product_id: 'p2', qty: '2.0000', unit_price: '33.3333' } },
      ],
    },
  } as never;
  const posted = (id: string, lines: [string, string][]) =>
    ({
      id,
      lines: {
        sales_return_line: lines.map(([line, qty]) => ({
          id: `${id}-${line}`,
          data: { shipment_line_id: line, qty },
        })),
      },
    }) as never;

  it('only posted shipments can be returned', () => {
    expect(shipmentNotReturnable('posted')).toBeUndefined();
    expect(shipmentNotReturnable('draft')).toBe('出貨單尚未過帳');
    expect(shipmentNotReturnable('void')).toBe('出貨單已作廢');
  });

  it('returnable qty subtracts other posted returns (§8.9 example: 12 → 8 → 4)', () => {
    const returns = [posted('r1', [['s1', '8.0000']]), posted('r2', [['s2', '2.0000']])];
    const returnable = salesReturnableQty(shipment, returns);
    expect([...returnable]).toEqual([
      ['s1', '4.0000'],
      ['s2', '0.0000'],
    ]);
    // 修改已過帳的退回本身時不計入自己
    expect(salesReturnableQty(shipment, returns, 'r1').get('s1')).toBe('12.0000');
    expect(returnLinesFromShipment(shipment, returnable)).toEqual([
      {
        shipment_line_id: 's1',
        product_id: 'p1',
        product_name: null,
        unit_price: '120.0000',
        discount_rate: '0.1500',
        qty: '4.0000',
      },
    ]);
  });

  it('lists the lots added back by a posted return', () => {
    const txn = {
      id: 't1',
      txn_type: 'sales_return',
      product_id: 'p1',
      product_code: 'A001',
      product_name: '螺絲',
      lot_no: 'L-02',
      qty_change: '7.0000',
      unit_cost_twd: '82.5000',
      ref_line_id: 'a',
    } as InventoryTransaction;
    expect(usedLots([txn], ['a'], 'sales_return')).toMatchObject([
      { row: 0, lotNo: 'L-02', qty: '7.0000', costTwd: '577.50' },
    ]);
    expect(usedLots([txn], ['a'])).toEqual([]);
  });
});

describe('product names (M11)', () => {
  it('order and shipment lines carry product names, returns do not', () => {
    expect(namedSalesLine('sales_order_line')).toBe(true);
    expect(namedSalesLine('shipment_line')).toBe(true);
    expect(namedSalesLine('sales_return_line')).toBe(false);
    expect(namedSalesLine('goods_receipt_line')).toBe(false);
  });

  it('uses the lookup product name, falls back to the product name', () => {
    const items = [
      { id: 'p1', product_name: 'X 型扳手' },
      { id: 'p2', product_name: '鉗子' },
    ];
    expect(productNameOf(items, 'p1', '扳手 10mm')).toBe('X 型扳手');
    expect(productNameOf(items, 'p3', '螺絲')).toBe('螺絲');
    expect(productNameOf(undefined, 'p1', '扳手 10mm')).toBe('扳手 10mm');
    expect(productNameOf([], 'p1', undefined)).toBeUndefined();
  });
});

describe('lineProductNames (換客戶重新帶入)', () => {
  it('maps each line by product id, keeping lines without a result', () => {
    const names = [
      { product_id: 'P001', product_name: 'X 型扳手' },
      // 料號有包含關係（P001、P0011）也以 id 對應
      { product_id: 'P0011', product_name: '扳手組' },
    ];
    expect(lineProductNames(['P0011', undefined, 'P001', 'P009', 'P001'], names)).toEqual([
      '扳手組',
      undefined,
      'X 型扳手',
      undefined,
      'X 型扳手',
    ]);
    expect(lineProductNames([], names)).toEqual([]);
  });
});
