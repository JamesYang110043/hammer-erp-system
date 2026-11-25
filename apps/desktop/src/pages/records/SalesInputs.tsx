import { useQueryClient } from '@tanstack/react-query';
import { App, Button, Form, Input, Modal, Select, Space, Spin, Table, Typography } from 'antd';
import { useContext, useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { Link } from 'react-router';

import { StatusTag } from '../../components/StatusTag';
import { SectionTitle } from '../../components/SectionTitle';
import { ShipmentAllocationContext } from './shipmentAllocation';

import { unwrap } from '../../api/client';
import { useTaxRates } from '../../api/currencies';
import { useProductLots } from '../../api/inventory';
import { MAX_PAGE_SIZE, recordKey, useRecord } from '../../api/records';
import {
  salesOrderQuery,
  salesProductLookupQuery,
  shipmentReturnableQuery,
  useDocumentLotTransactions,
  useSalesProductLookup,
  useShipmentReturnable,
} from '../../api/sales';
import { commands } from '../../bindings';
import type { Record as DataRecord, FieldDefinition, RelationRef } from '../../bindings';
import { relationLabel } from '../../records/display';
import { useDebouncedValue } from '../../utils/useDebouncedValue';
import { LOOKUP_DEBOUNCE_MS, RelationSelect } from './RelationSelect';
import { documentStatus, formatQty } from '../../records/documents';
import { initialFormValues } from '../../records/form';
import { formatUnitCost, productStockPath } from '../../records/inventory';
import { LINES_FORM_KEY } from '../../records/lines';
import type { DetailTable, LineFormValues } from '../../records/lines';
import { groupThousands } from '../../records/display';
import {
  SALES_FIELDS as F,
  SALES_RETURN,
  SALES_RETURN_LINE,
  SHIPMENT,
  SHIPMENT_LINE,
  lotPicksError,
  namedSalesLine,
  orderNotShippable,
  productNameOf,
  returnLinesFromShipment,
  salePrice,
  salesLineKey,
  shipmentLinesFromOrder,
  shipmentNotReturnable,
  splitByLots,
  usedLots,
} from '../../records/sales';
import type { LotPick, RowAllocation } from '../../records/sales';

/**
 * 訂單、出貨單、銷貨退回（docs/architecture.md §8.9）在動態表單中的專屬輸入與自動帶入：
 *
 * - {@link SalesAutofill}：明細選商品帶入售價與品名、換客戶時詢問是否重新帶入品名；出貨單選客戶帶入
 *   送貨地址、選訂單帶入未出貨明細
 * - {@link SalesProductSelect}：訂單、出貨明細的商品選擇器，可用客戶品名搜尋（§8.9「銷貨用商品選擇 API」）
 * - {@link useShipmentAllocation}、{@link ShipmentLotCell}：出貨明細每一列的建議批號（先進先出試算）
 *   與手動批號的可用數量，可以開啟 {@link LotPickerModal} 指定批號（拆成每個批號一列）
 * - {@link UsedLotsTable}：出貨單過帳後實際扣用的批號、銷貨退回過帳後加回的批號與成本
 * - {@link SalesReturnAutofill}、{@link ShipmentReturnableQty}：銷貨退回選擇出貨單後帶入可退的明細、
 *   顯示每一列的可退數量
 */

function textOf(value: unknown): string | undefined {
  return typeof value === 'string' && value !== '' ? value : undefined;
}

type Rows = LineFormValues[];

/**
 * 依使用者的修改自動帶入（不顯示內容，放在 antd `Form` 內）。只有可以修改的草稿使用。
 *
 * - 明細選了商品：帶入商品的預設售價（單價含稅時換算含稅價）與品名（這位客戶的客戶品名 → 商品名稱，
 *   §8.9「明細的品名」）；換商品時重新帶入
 * - 表頭換客戶、且明細已有商品：詢問是否依新客戶重新帶入所有明細的品名（已手動修改的也會被覆蓋）
 * - 出貨單選了客戶：帶入客戶的地址到送貨地址
 * - 出貨單選了訂單：帶入客戶、稅別、單價含稅、送貨地址與未出貨的明細（§8.9「訂單轉出貨單」）
 *
 * 自動帶入的值同時寫進比較基準，不會再觸發下一個帶入。寫入表單後呼叫 `onChange`。
 */
export function SalesAutofill({
  entityKey,
  details,
  onChange,
}: {
  entityKey: string;
  details: readonly DetailTable[];
  onChange: () => void;
}) {
  const form = Form.useFormInstance();
  const queryClient = useQueryClient();
  const { message, modal } = App.useApp();
  const taxRates = useTaxRates();
  const lineKey = salesLineKey(entityKey)!;
  const linePath = [LINES_FORM_KEY, lineKey];
  const isShipment = entityKey === SHIPMENT;

  const customer = textOf(Form.useWatch(F.customer, form));
  const order = textOf(Form.useWatch(F.salesOrder, form));
  const rows = Form.useWatch(linePath, form) as Rows | undefined;
  const products = (rows ?? []).map((r) => textOf(r?.[F.product]));

  const prev = useRef({
    customer: textOf(form.getFieldValue(F.customer)),
    order: textOf(form.getFieldValue(F.salesOrder)),
    products: ((form.getFieldValue(linePath) ?? []) as Rows).map((r) => textOf(r?.[F.product])),
  });

  const currentProducts = () =>
    ((form.getFieldValue(linePath) ?? []) as Rows).map((r) => textOf(r?.[F.product]));

  const fetchRecord = (key: string, id: string) =>
    queryClient
      .fetchQuery({
        queryKey: recordKey(key, id),
        queryFn: () => unwrap(commands.getRecord(key, id)),
        staleTime: 0,
      })
      .catch(() => undefined as DataRecord | undefined);

  /** 帶入送貨地址（客戶有地址時）。 */
  const applyCustomer = async (id: string) => {
    const record = await fetchRecord('customer', id);
    const address = textOf(record?.data.address);
    if (address) {
      form.setFieldValue(F.shipAddress, address);
      onChange();
    }
  };

  /**
   * 商品 `product` 對客戶 `customerId` 要帶入的品名：以選擇 API 用料號查詢（取得客戶品名），
   * 找不到或沒有客戶時用商品名稱。
   */
  const productName = async (customerId: string | undefined, product: DataRecord | undefined) => {
    const fallback = textOf(product?.data.name);
    const code = textOf(product?.data.code);
    if (!product || !customerId || !code) {
      return fallback;
    }
    const items = await queryClient
      .fetchQuery({ ...salesProductLookupQuery(customerId, code, 50), staleTime: 0 })
      .catch(() => undefined);
    return productNameOf(items, product.id, fallback);
  };

  /** 明細第 `index` 列選了商品：帶入預設售價與品名。 */
  const applyProduct = async (index: number, id: string) => {
    const product = await fetchRecord('product', id);
    const taxIncluded = form.getFieldValue(F.taxIncluded) === true;
    const rate = taxRates.data?.find((t) => t.code === form.getFieldValue(F.taxType))?.rate;
    const price = salePrice(product?.data.default_sale_price, taxIncluded, rate);
    const name = namedSalesLine(lineKey)
      ? await productName(textOf(form.getFieldValue(F.customer)), product)
      : undefined;
    // 等待期間該列可能已換成別的商品
    if (currentProducts()[index] !== id || (price === undefined && name === undefined)) {
      return;
    }
    if (price !== undefined) {
      form.setFieldValue([...linePath, index, F.unitPrice], price);
    }
    if (name !== undefined) {
      form.setFieldValue([...linePath, index, F.productName], name);
    }
    onChange();
  };

  /** 換了客戶：依新客戶重新帶入所有已選商品的明細的品名。 */
  const applyCustomerNames = async (customerId: string) => {
    const ids = currentProducts();
    const names = await Promise.all(
      ids.map(async (id) =>
        id ? productName(customerId, await fetchRecord('product', id)) : undefined,
      ),
    );
    // 等待期間又換了客戶或商品
    if (textOf(form.getFieldValue(F.customer)) !== customerId) {
      return;
    }
    const after = currentProducts();
    names.forEach((name, i) => {
      if (name !== undefined && after[i] === ids[i]) {
        form.setFieldValue([...linePath, i, F.productName], name);
      }
    });
    onChange();
  };

  /** 表頭換了客戶、且明細已有商品時，詢問是否重新帶入品名。 */
  const confirmCustomerNames = (customerId: string) => {
    if (!namedSalesLine(lineKey) || !currentProducts().some(Boolean)) {
      return;
    }
    modal.confirm({
      title: '是否依新客戶重新帶入品名？',
      content: '已手動修改的品名也會被覆蓋。',
      okText: '重新帶入',
      cancelText: '不變',
      onOk: () => applyCustomerNames(customerId),
    });
  };

  /** 出貨單選了訂單：帶入表頭與未出貨的明細。 */
  const applyOrder = async (id: string | undefined) => {
    const table = details.find((d) => d.entity.key === SHIPMENT_LINE);
    if (!table) {
      return;
    }
    const current = (form.getFieldValue(linePath) ?? []) as Rows;
    if (!id) {
      // 取消訂單：明細不再指向訂單明細
      if (current.some((r) => r[F.salesOrderLine])) {
        form.setFieldValue(
          linePath,
          current.map((r) => ({ ...r, [F.salesOrderLine]: undefined })),
        );
        onChange();
      }
      return;
    }
    const so = await queryClient
      .fetchQuery({ ...salesOrderQuery(id), staleTime: 0 })
      .catch(() => undefined);
    if (!so) {
      message.error('無法讀取訂單');
      return;
    }
    const reason = orderNotShippable(documentStatus(so));
    if (reason) {
      message.warning(`${reason}，無法帶入明細`);
      return;
    }
    const lines = shipmentLinesFromOrder(so);
    const name = so.record_no ?? '訂單';
    const fill = async () => {
      const customerId = textOf(so.data[F.customer]);
      form.setFieldsValue({
        [F.customer]: customerId,
        [F.taxType]: so.data[F.taxType] ?? undefined,
        [F.taxIncluded]: so.data[F.taxIncluded] === true,
      });
      form.setFieldValue(
        linePath,
        lines.map((data) => initialFormValues(table.fields, data)),
      );
      prev.current = { customer: customerId, order: id, products: currentProducts() };
      if (customerId) {
        await applyCustomer(customerId);
      }
      onChange();
      message.success(
        lines.length > 0
          ? `已帶入 ${name} 的 ${lines.length} 筆未出貨明細`
          : `${name} 已沒有未出貨的明細`,
      );
    };
    if (current.length > 0) {
      modal.confirm({
        title: '要以訂單的明細取代目前的明細嗎？',
        content: `會帶入 ${name} 尚未出貨的明細，目前的 ${current.length} 筆明細會被取代。`,
        okText: '取代',
        cancelText: '保留目前的明細',
        onOk: fill,
      });
      return;
    }
    await fill();
  };

  const productKey = products.join('|');
  useEffect(() => {
    // Form.useWatch 第一次繪製可能是 undefined（欄位還沒註冊）：與表單目前的值相同才比較
    const actual = {
      customer: textOf(form.getFieldValue(F.customer)),
      order: textOf(form.getFieldValue(F.salesOrder)),
      products: currentProducts(),
    };
    if (
      customer !== actual.customer ||
      order !== actual.order ||
      productKey !== actual.products.join('|')
    ) {
      return;
    }
    const before = prev.current;
    prev.current = actual;
    if (isShipment && before.order !== order) {
      void applyOrder(order);
      return;
    }
    if (isShipment && before.customer !== customer && customer) {
      void applyCustomer(customer);
    }
    if (before.customer !== customer && customer) {
      confirmCustomerNames(customer);
    }
    // 列數不變時才比較（新增、刪除列會讓位置位移）
    if (before.products.length === actual.products.length) {
      actual.products.forEach((id, i) => {
        if (id && id !== before.products[i]) {
          void applyProduct(i, id);
        }
      });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [customer, order, productKey]);

  return null;
}

// ---- 商品選擇器 ----

/**
 * 訂單、出貨明細的商品選擇器（放在明細表格的 `Form.Item` 內，值為商品 id）。
 *
 * 表頭已選客戶時改用銷貨用商品選擇 API（§8.9）：可以用這位客戶的客戶品名搜尋，選項顯示「料號 商品名稱」，
 * 有客戶品名時另外顯示客戶品名。沒有選客戶時與一般的關聯選擇器（{@link RelationSelect}）相同。
 * 選定後的品名由 {@link SalesAutofill} 帶入。
 */
export function SalesProductSelect({
  field,
  value,
  onChange,
  initial,
  disabled,
  id,
  'aria-label': ariaLabel,
}: {
  field: FieldDefinition;
  value?: string;
  onChange?: (value: string | undefined) => void;
  initial?: RelationRef;
  disabled?: boolean;
  id?: string;
  'aria-label'?: string;
}) {
  const form = Form.useFormInstance();
  const customer = textOf(Form.useWatch(F.customer, form) as unknown);
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const q = useDebouncedValue(search.trim(), LOOKUP_DEBOUNCE_MS);
  const lookup = useSalesProductLookup(customer, q, open && customer !== undefined);
  // 選過的選項的標題，選單關閉、換了關鍵字後仍可顯示
  const [picked, setPicked] = useState<Map<string, string>>(() => new Map());
  const items = lookup.data ?? [];
  const known =
    value === undefined
      ? undefined
      : initial?.id === value
        ? relationLabel(initial)
        : (picked.get(value) ?? items.find((i) => i.id === value)?.title);
  // 不知道標題的值（例如從訂單帶入的明細）：查詢那一筆商品
  const resolved = useRecord('product', customer && value && !known ? value : undefined);

  if (!customer) {
    return (
      <RelationSelect
        field={field}
        value={value}
        onChange={onChange}
        initial={initial}
        disabled={disabled}
        id={id}
        aria-label={ariaLabel}
      />
    );
  }

  const options = items.map((item) => ({
    value: item.id,
    label: item.title,
    customerName: item.customer_product_name,
  }));
  if (value && !options.some((o) => o.value === value)) {
    const label =
      known ??
      (resolved.data
        ? resolved.data.title
        : resolved.isError
          ? relationLabel({ title: '', deleted: true })
          : '載入中…');
    options.unshift({ value, label, customerName: null });
  }

  return (
    <Select
      id={id}
      aria-label={ariaLabel}
      disabled={disabled}
      style={{ maxWidth: 320, width: '100%' }}
      showSearch
      allowClear
      placeholder="搜尋商品或客戶品名"
      filterOption={false}
      searchValue={search}
      onSearch={setSearch}
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) {
          setSearch('');
        }
      }}
      loading={lookup.isFetching}
      notFoundContent={lookup.isFetching ? <Spin size="small" /> : '找不到資料'}
      value={value || undefined}
      options={options}
      optionLabelProp="label"
      optionRender={(option) => (
        <div>
          <div>{option.data.label}</div>
          {option.data.customerName ? (
            <Typography.Text type="secondary" style={{ fontSize: 12 }}>
              客戶品名：{option.data.customerName}
            </Typography.Text>
          ) : null}
        </div>
      )}
      onChange={(next?: string) => {
        const item = items.find((i) => i.id === next);
        if (next && item) {
          setPicked((m) => new Map(m).set(next, item.title));
        }
        onChange?.(next);
      }}
    />
  );
}

// ---- 出貨的建議批號 ----

/** 建議批號的文字：`L-01 × 5、L-02 × 7`。 */
function allocationText(a: RowAllocation): string {
  return a.allocations.map((x) => `${x.lot_no} × ${formatQty(x.qty)}`).join('、');
}

/**
 * 出貨明細的「建議批號」欄：
 *
 * - 批號留空：先進先出的試算結果；不足時標示不足的數量
 * - 手動指定批號：該批號的可用數量是否足夠
 * - 服務類商品：不需要批號
 *
 * 「指定批號」開啟 {@link LotPickerModal}，確認後把該列拆成每個批號一列。
 */
export function ShipmentLotCell({
  listName,
  row,
  onRowsChange,
}: {
  listName: (string | number)[];
  row: number;
  onRowsChange: () => void;
}) {
  const form = Form.useFormInstance();
  const ctx = useContext(ShipmentAllocationContext);
  const productId = textOf(Form.useWatch([...listName, row, F.product], form));
  const qty = Form.useWatch([...listName, row, F.qty], form) as unknown;
  const [open, setOpen] = useState(false);
  const a = ctx.allocations.get(row);
  const errors = ctx.errors.filter((e) => e.row === row);

  let content: ReactNode;
  if (errors.length > 0) {
    content = (
      <Typography.Text type="danger">{errors.map((e) => e.message).join('；')}</Typography.Text>
    );
  } else if (!a) {
    content = (
      <Typography.Text type="secondary">
        {ctx.loading ? '試算中…' : (ctx.failed ?? '—')}
      </Typography.Text>
    );
  } else if (a.productType === 'service') {
    content = <Typography.Text type="secondary">服務類商品，不需要批號</Typography.Text>;
  } else {
    content = (
      <Space size={4} wrap>
        <StatusTag tone={a.manual ? 'info' : 'neutral'}>{a.manual ? '手動' : '先進先出'}</StatusTag>
        {a.allocations.length > 0 && <Typography.Text>{allocationText(a)}</Typography.Text>}
        {a.shortage && (
          <Typography.Text type="danger">不足 {formatQty(a.shortage)}</Typography.Text>
        )}
      </Space>
    );
  }
  const canPick = Boolean(productId) && a?.productType !== 'service';
  return (
    <Space direction="vertical" size={2} data-testid={`allocation-${row}`}>
      {content}
      {canPick && (
        <Button size="small" type="link" style={{ padding: 0 }} onClick={() => setOpen(true)}>
          指定批號
        </Button>
      )}
      {open && productId && (
        <LotPickerModal
          productId={productId}
          lineQty={qty}
          suggestion={a}
          currentLotNo={textOf(form.getFieldValue([...listName, row, F.lotNo]))}
          rowNumber={row + 1}
          onCancel={() => setOpen(false)}
          onOk={(picks) => {
            const rows = (form.getFieldValue(listName) ?? []) as Rows;
            const split = splitByLots(rows[row] ?? {}, picks);
            form.setFieldValue(listName, [...rows.slice(0, row), ...split, ...rows.slice(row + 1)]);
            setOpen(false);
            onRowsChange();
          }}
        />
      )}
    </Space>
  );
}

/**
 * 「指定批號」對話框：列出商品目前有數量的批號（先進先出順序），預先填入建議批號（或目前指定的批號），
 * 使用者修改各批號的數量；合計必須等於該列的數量、每個批號不超過剩餘數量，才能套用。
 */
function LotPickerModal({
  productId,
  lineQty,
  suggestion,
  currentLotNo,
  rowNumber,
  onCancel,
  onOk,
}: {
  productId: string;
  lineQty: unknown;
  suggestion: RowAllocation | undefined;
  currentLotNo: string | undefined;
  rowNumber: number;
  onCancel: () => void;
  onOk: (picks: LotPick[]) => void;
}) {
  const lots = useProductLots(productId, { page_size: MAX_PAGE_SIZE });
  const [qtys, setQtys] = useState<Record<string, string>>(() => {
    if (currentLotNo && typeof lineQty === 'string') {
      return { [currentLotNo.toLowerCase()]: lineQty };
    }
    return Object.fromEntries(
      (suggestion?.allocations ?? []).map((x) => [x.lot_no.toLowerCase(), trimZeros(x.qty)]),
    );
  });
  const items = lots.data?.items ?? [];
  const picks: LotPick[] = items.map((lot) => ({
    lot_no: lot.lot_no,
    qty: qtys[lot.lot_no.toLowerCase()] ?? '',
  }));
  const remaining = new Map(items.map((lot) => [lot.lot_no.toLowerCase(), lot.qty_remaining]));
  const error = lots.data ? lotPicksError(picks, lineQty, remaining) : undefined;
  const need = typeof lineQty === 'string' ? formatQty(lineQty) : '—';
  return (
    <Modal
      open
      width={720}
      title={`指定批號（明細第 ${rowNumber} 列）`}
      okText="套用"
      cancelText="取消"
      okButtonProps={{ disabled: !lots.data || error !== undefined }}
      onCancel={onCancel}
      onOk={() => onOk(picks)}
    >
      <Typography.Paragraph type="secondary">
        出貨數量 {need}
        。填入要從各批號出貨的數量，套用後會拆成每個批號一列；清空批號欄位即改回先進先出。
      </Typography.Paragraph>
      <Table
        size="small"
        bordered
        rowKey="id"
        pagination={false}
        loading={lots.isLoading}
        dataSource={items}
        locale={{ emptyText: '此商品目前沒有庫存' }}
        columns={[
          { key: 'lot_no', title: '批號', dataIndex: 'lot_no' },
          { key: 'received_date', title: '入庫日期', dataIndex: 'received_date' },
          {
            key: 'qty_remaining',
            title: '剩餘數量',
            align: 'right',
            render: (_, lot) => formatQty(lot.qty_remaining),
          },
          {
            key: 'unit_cost_twd',
            title: '單位成本',
            align: 'right',
            render: (_, lot) => formatUnitCost(lot.unit_cost_twd),
          },
          {
            key: 'pick',
            title: '出貨數量',
            width: 140,
            render: (_, lot) => (
              <Input
                aria-label={`批號 ${lot.lot_no} 出貨數量`}
                inputMode="decimal"
                value={qtys[lot.lot_no.toLowerCase()] ?? ''}
                onChange={(e) =>
                  setQtys((q) => ({ ...q, [lot.lot_no.toLowerCase()]: e.target.value.trim() }))
                }
              />
            ),
          },
        ]}
      />
      {error && (
        <Typography.Text type="danger" style={{ display: 'block', marginTop: 8 }}>
          {error}
        </Typography.Text>
      )}
    </Modal>
  );
}

function trimZeros(raw: string): string {
  return raw.includes('.') ? raw.replace(/\.?0+$/, '') : raw;
}

// ---- 過帳後扣用（加回）的批號 ----

const LOT_TABLE = {
  [SHIPMENT]: {
    lineKey: SHIPMENT_LINE,
    title: '實際扣用的批號',
    empty: '沒有扣用批號（服務類商品不扣庫存）',
  },
  [SALES_RETURN]: {
    lineKey: SALES_RETURN_LINE,
    title: '加回的批號',
    empty: '沒有加回批號（服務類商品不處理庫存）',
  },
} as const;

/**
 * 出貨單過帳後實際扣用的批號、銷貨退回過帳後加回的批號（原出貨扣除的批號），
 * 列出數量、單位成本與成本，依明細順序。
 */
export function UsedLotsTable({
  record,
  docType,
}: {
  record: DataRecord;
  docType: typeof SHIPMENT | typeof SALES_RETURN;
}) {
  const config = LOT_TABLE[docType];
  const txns = useDocumentLotTransactions(docType, record.id);
  const lineIds = (record.lines?.[config.lineKey] ?? []).map((l) => l.id);
  const items = usedLots(txns.data?.items ?? [], lineIds, docType);
  return (
    <section aria-label={config.title} style={{ marginBottom: 24 }}>
      <SectionTitle>{config.title}</SectionTitle>
      <Table
        size="small"
        bordered
        pagination={false}
        loading={txns.isLoading}
        rowKey={(u) => `${u.lineId}:${u.lotNo}`}
        dataSource={items}
        locale={{ emptyText: config.empty }}
        columns={[
          {
            key: 'row',
            title: '明細',
            align: 'right',
            width: 64,
            render: (_, u) => (u.row !== undefined ? `第 ${u.row + 1} 列` : '—'),
          },
          { key: 'product', title: '商品', dataIndex: 'product' },
          {
            key: 'lot',
            title: '批號',
            render: (_, u) => (
              <Link to={productStockPath(u.productId)} title="查看此商品的批號與庫存">
                {u.lotNo}
              </Link>
            ),
          },
          { key: 'qty', title: '數量', align: 'right', render: (_, u) => formatQty(u.qty) },
          {
            key: 'unit_cost',
            title: '單位成本',
            align: 'right',
            render: (_, u) => formatUnitCost(u.unitCostTwd),
          },
          {
            key: 'cost',
            title: '成本',
            align: 'right',
            render: (_, u) => groupThousands(u.costTwd),
          },
        ]}
      />
    </section>
  );
}

// ---- 銷貨退回 ----

/**
 * 銷貨退回選了出貨單：帶入客戶、稅別、單價含稅（系統維護，顯示與金額預覽用）與可退數量大於 0 的明細
 * （§8.9「銷貨退回」）。已有明細時先詢問是否取代。只有新增中的草稿使用（存檔後不可更換出貨單）。
 */
export function SalesReturnAutofill({
  details,
  onChange,
}: {
  details: readonly DetailTable[];
  onChange: () => void;
}) {
  const form = Form.useFormInstance();
  const queryClient = useQueryClient();
  const { message, modal } = App.useApp();
  const shipmentId = textOf(Form.useWatch(F.shipment, form));
  const prev = useRef(textOf(form.getFieldValue(F.shipment)));

  const apply = async (id: string) => {
    const table = details.find((d) => d.entity.key === SALES_RETURN_LINE);
    if (!table) {
      return;
    }
    const result = await queryClient
      .fetchQuery({ ...shipmentReturnableQuery(id), staleTime: 0 })
      .catch(() => undefined);
    if (!result) {
      message.error('無法讀取出貨單');
      return;
    }
    const { shipment, returnable } = result;
    const reason = shipmentNotReturnable(documentStatus(shipment));
    if (reason) {
      message.warning(`${reason}，無法退回`);
      return;
    }
    const lines = returnLinesFromShipment(shipment, returnable);
    const name = shipment.record_no ?? '出貨單';
    const path = [LINES_FORM_KEY, SALES_RETURN_LINE];
    const fill = () => {
      form.setFieldsValue({
        [F.customer]: shipment.data[F.customer] ?? undefined,
        [F.taxType]: shipment.data[F.taxType] ?? undefined,
        [F.taxIncluded]: shipment.data[F.taxIncluded] === true,
      });
      form.setFieldValue(
        path,
        lines.map((data) => initialFormValues(table.fields, data)),
      );
      onChange();
      if (lines.length > 0) {
        message.success(
          `已帶入 ${name} 的 ${lines.length} 筆可退明細，請修改退回數量或刪除不退的列`,
        );
      } else {
        message.warning(`${name} 已沒有可退的數量`);
      }
    };
    const rows = (form.getFieldValue(path) ?? []) as unknown[];
    if (rows.length > 0) {
      modal.confirm({
        title: '要以出貨單的明細取代目前的明細嗎？',
        content: `會帶入 ${name} 可退的明細，目前的 ${rows.length} 筆明細會被取代。`,
        okText: '取代',
        cancelText: '保留目前的明細',
        onOk: fill,
      });
      return;
    }
    fill();
  };

  useEffect(() => {
    // Form.useWatch 第一次繪製可能是 undefined（欄位還沒註冊）：與表單目前的值相同才比較
    if (shipmentId !== textOf(form.getFieldValue(F.shipment))) {
      return;
    }
    const before = prev.current;
    prev.current = shipmentId;
    if (shipmentId && shipmentId !== before) {
      void apply(shipmentId);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shipmentId]);

  return null;
}

/** 銷貨退回明細的可退數量：出貨明細的數量減去其他已過帳退回的數量。 */
export function ShipmentReturnableQty({ row }: { row: (string | number)[] }) {
  const form = Form.useFormInstance();
  const shipmentId = textOf(Form.useWatch(F.shipment, form));
  const lineId = textOf(Form.useWatch([...row, F.shipmentLine], form));
  const result = useShipmentReturnable(shipmentId);
  const qty = lineId ? result.data?.returnable.get(lineId) : undefined;
  return qty ? (
    <Typography.Text strong>{formatQty(qty)}</Typography.Text>
  ) : (
    <Typography.Text type="secondary">—</Typography.Text>
  );
}
