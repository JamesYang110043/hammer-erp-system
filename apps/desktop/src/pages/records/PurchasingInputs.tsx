import { useQueryClient } from '@tanstack/react-query';
import { App, Form, Input, Space, Typography } from 'antd';
import dayjs from 'dayjs';
import { useEffect, useRef } from 'react';
import { Link } from 'react-router';

import { unwrap } from '../../api/client';
import { allCurrenciesQuery, useAllCurrencies } from '../../api/currencies';
import { effectiveRateQuery, useEffectiveRate } from '../../api/exchangeRates';
import { returnableQuery, useReturnable } from '../../api/purchasing';
import { recordKey } from '../../api/records';
import { commands } from '../../bindings';
import type { Currency, FieldDefinition } from '../../bindings';
import { documentStatus, formatQty } from '../../records/documents';
import { initialFormValues } from '../../records/form';
import { productStockPath } from '../../records/inventory';
import { LINES_FORM_KEY } from '../../records/lines';
import type { DetailTable, LineFormValues } from '../../records/lines';
import {
  GOODS_RECEIPT,
  PURCHASE_ORDER,
  PURCHASE_RETURN_LINE,
  PURCHASING_FIELDS as F,
  defaultTaxType,
  orderNotReceivable,
  pricedDocument,
  rateNote,
  receiptLinesFromOrder,
  receiptNotReturnable,
  returnLinesFromReceipt,
  sameDecimal,
  trimRate,
} from '../../records/purchasing';
import { RecordFieldInput } from './RecordFieldInput';

/**
 * 採購單、進貨單（docs/architecture.md §8.8）在動態表單中的專屬輸入與自動帶入：
 *
 * - {@link ExchangeRateInput}：匯率輸入，下方顯示匯率來源日期，找不到匯率時提示
 * - {@link PurchasingAutofill}：選廠商帶入幣別、換幣別帶入匯率與稅別、換單據日期詢問是否套用新的匯率、
 *   進貨單選採購單帶入未進貨明細
 * - `useAmountPreview`（另一個檔案）：明細金額與合計的即時預覽
 * - {@link ReceiptLotNoInput}：進貨明細的批號（留空時過帳自動產生；過帳後連到庫存查詢）
 * - {@link PurchaseReturnAutofill}、{@link ReturnableQty}：進貨退回選擇進貨單後帶入可退的明細、
 *   顯示每一列的可退數量
 */

/** 表單中日期欄位的值（Dayjs）→ `YYYY-MM-DD`。 */
function dateText(value: unknown): string | undefined {
  return dayjs.isDayjs(value) && value.isValid() ? value.format('YYYY-MM-DD') : undefined;
}

function textOf(value: unknown): string | undefined {
  return typeof value === 'string' && value !== '' ? value : undefined;
}

function findCurrency(currencies: readonly Currency[] | undefined, code: string | undefined) {
  return code ? currencies?.find((c) => c.code === code) : undefined;
}

/** 匯率：輸入框，下方顯示適用匯率的日期；找不到匯率、或與適用匯率不同時提示。 */
export function ExchangeRateInput({
  entityKey,
  field,
  disabled,
  value,
  onChange,
  id,
}: {
  entityKey: string;
  field: FieldDefinition;
  disabled?: boolean;
  value?: unknown;
  onChange?: (value: unknown) => void;
  id?: string;
}) {
  const form = Form.useFormInstance();
  const doc = pricedDocument(entityKey);
  const currency = textOf(Form.useWatch(F.currency, form));
  const date = dateText(Form.useWatch(doc?.dateField ?? '', form));
  const currencies = useAllCurrencies();
  const isBase = findCurrency(currencies.data, currency)?.is_base ?? false;
  // 唯讀（已確認、已過帳）時匯率已鎖定，不再查詢
  const effective = useEffectiveRate(disabled || isBase ? undefined : currency, date);
  const note = disabled ? undefined : rateNote(effective.data, value, isBase);
  return (
    <Space direction="vertical" size={2} style={{ width: '100%' }}>
      <RecordFieldInput
        id={id}
        field={field}
        disabled={disabled || isBase || undefined}
        value={value}
        onChange={onChange}
      />
      {note && (
        <Typography.Text type={note.type} data-testid="exchange-rate-note">
          {note.text}
        </Typography.Text>
      )}
    </Space>
  );
}

/** 表單中要監看的值，與上一次比較用的文字。 */
interface Watched {
  supplier?: string;
  currency?: string;
  date?: string;
  order?: string;
}

/**
 * 依使用者的修改自動帶入（不顯示內容，放在 antd `Form` 內）。只有可以修改的草稿使用。
 *
 * 只回應使用者的修改：自動帶入的值同時寫進比較基準，不會再觸發下一個帶入。
 * 寫入表單後呼叫 `onChange`（`setFieldValue` 不會觸發 Form 的 `onValuesChange`）。
 */
export function PurchasingAutofill({
  entityKey,
  isNew,
  fields,
  details,
  onChange,
}: {
  entityKey: string;
  /** 新增中（還沒存檔）。 */
  isNew: boolean;
  fields: readonly FieldDefinition[];
  details: readonly DetailTable[];
  onChange: () => void;
}) {
  const form = Form.useFormInstance();
  const queryClient = useQueryClient();
  const { message, modal } = App.useApp();
  const doc = pricedDocument(entityKey)!;
  const taxHasDefault = fields.some(
    (f) => f.field_key === F.taxType && f.default_value !== null && f.default_value !== undefined,
  );

  const read = (): Watched => ({
    supplier: textOf(form.getFieldValue(F.supplier)),
    currency: textOf(form.getFieldValue(F.currency)),
    date: dateText(form.getFieldValue(doc.dateField)),
    order: textOf(form.getFieldValue(F.purchaseOrder)),
  });
  const prev = useRef<Watched>(read());

  const supplier = textOf(Form.useWatch(F.supplier, form));
  const currency = textOf(Form.useWatch(F.currency, form));
  const date = dateText(Form.useWatch(doc.dateField, form));
  const order = textOf(Form.useWatch(F.purchaseOrder, form));

  /** 寫入表單，並更新比較基準（不觸發下一個帶入）。 */
  const setValues = (values: Record<string, unknown>) => {
    for (const [key, value] of Object.entries(values)) {
      form.setFieldValue(key, value);
    }
    prev.current = read();
  };

  const fetchRate = (code: string, day: string) =>
    queryClient.fetchQuery(effectiveRateQuery(code, day)).catch(() => undefined);
  const fetchCurrencies = () =>
    queryClient.fetchQuery(allCurrenciesQuery).catch(() => undefined as Currency[] | undefined);

  /** 幣別變動：本位幣匯率為 1，外幣帶入適用匯率（找不到時清空）；新增時稅別依幣別帶入預設值。 */
  const applyCurrency = async (code: string, taxType?: string) => {
    const info = findCurrency(await fetchCurrencies(), code);
    const isBase = info?.is_base ?? false;
    const values: Record<string, unknown> = {};
    if (taxType !== undefined) {
      values[F.taxType] = taxType;
    } else if ((isNew || !textOf(form.getFieldValue(F.taxType))) && !taxHasDefault) {
      values[F.taxType] = defaultTaxType(isBase);
    }
    const day = dateText(form.getFieldValue(doc.dateField));
    if (isBase) {
      values[F.rate] = '1';
    } else if (day) {
      const rate = await fetchRate(code, day);
      values[F.rate] = rate?.rate ? trimRate(rate.rate) : undefined;
    } else {
      values[F.rate] = undefined;
    }
    setValues(values);
    onChange();
  };

  /** 選了廠商：帶入廠商的預設幣別（進貨單有採購單時以採購單為準，不帶入）。 */
  const applySupplier = async (id: string) => {
    if (entityKey === GOODS_RECEIPT && textOf(form.getFieldValue(F.purchaseOrder))) {
      return;
    }
    const record = await queryClient
      .fetchQuery({
        queryKey: recordKey('supplier', id),
        queryFn: () => unwrap(commands.getRecord('supplier', id)),
      })
      .catch(() => undefined);
    const code = textOf(record?.data.default_currency_code);
    if (!code || code === textOf(form.getFieldValue(F.currency))) {
      return;
    }
    setValues({ [F.currency]: code });
    await applyCurrency(code);
  };

  /** 單據日期變動：查詢新的適用匯率，與目前的匯率不同時詢問是否套用。 */
  const applyDate = async (day: string) => {
    const code = textOf(form.getFieldValue(F.currency));
    if (!code) {
      return;
    }
    const info = findCurrency(await fetchCurrencies(), code);
    if (info?.is_base) {
      return;
    }
    const rate = await fetchRate(code, day);
    if (!rate?.rate) {
      return;
    }
    const current = form.getFieldValue(F.rate) as unknown;
    const next = trimRate(rate.rate);
    if (current === undefined || current === null || current === '') {
      setValues({ [F.rate]: next });
      onChange();
      return;
    }
    if (sameDecimal(current, rate.rate)) {
      return;
    }
    modal.confirm({
      title: '要套用新的匯率嗎？',
      content: `${code} 在 ${day} 的適用匯率為 ${next}（${rate.rate_date} 的匯率），目前的匯率為 ${String(current)}。`,
      okText: '套用',
      cancelText: '保留目前的匯率',
      onOk: () => {
        setValues({ [F.rate]: next });
        onChange();
      },
    });
  };

  /** 進貨單選了採購單：帶入廠商、幣別、稅別與未進貨的明細（§8.8「採購單轉進貨單」）。 */
  const applyOrder = async (id: string | undefined) => {
    const lineTable = details.find((d) => d.entity.key === doc.lineKey);
    if (!lineTable) {
      return;
    }
    const path = [LINES_FORM_KEY, doc.lineKey];
    const rows = (form.getFieldValue(path) ?? []) as LineFormValues[];
    if (!id) {
      // 取消採購單：明細不再指向採購明細
      if (rows.some((r) => r[F.purchaseOrderLine])) {
        setValues({});
        form.setFieldValue(
          path,
          rows.map((r) => ({ ...r, [F.purchaseOrderLine]: undefined })),
        );
        onChange();
      }
      return;
    }
    const po = await queryClient
      .fetchQuery({
        queryKey: recordKey(PURCHASE_ORDER, id),
        queryFn: () => unwrap(commands.getRecord(PURCHASE_ORDER, id)),
        staleTime: 0,
      })
      .catch(() => undefined);
    if (!po) {
      message.error('無法讀取採購單');
      return;
    }
    const reason = orderNotReceivable(documentStatus(po));
    if (reason) {
      message.warning(`${reason}，無法帶入明細`);
      return;
    }
    const lines = receiptLinesFromOrder(po);
    const apply = async () => {
      const code = textOf(po.data[F.currency]);
      setValues({
        [F.supplier]: po.data[F.supplier] ?? undefined,
        [F.currency]: code,
      });
      form.setFieldValue(
        path,
        lines.map((data) => initialFormValues(lineTable.fields, data)),
      );
      if (code) {
        await applyCurrency(code, textOf(po.data[F.taxType]));
      } else {
        onChange();
      }
      message.success(
        lines.length > 0
          ? `已帶入 ${po.record_no ?? '採購單'} 的 ${lines.length} 筆未進貨明細`
          : `${po.record_no ?? '採購單'} 已沒有未進貨的明細`,
      );
    };
    if (rows.length > 0) {
      modal.confirm({
        title: '要以採購單的明細取代目前的明細嗎？',
        content: `會帶入 ${po.record_no ?? '採購單'} 尚未進貨的明細，目前的 ${rows.length} 筆明細會被取代。`,
        okText: '取代',
        cancelText: '保留目前的明細',
        onOk: apply,
      });
      return;
    }
    await apply();
  };

  useEffect(() => {
    const before = prev.current;
    const now = { supplier, currency, date, order };
    // Form.useWatch 第一次繪製可能是 undefined（欄位還沒註冊）：與表單目前的值相同才比較
    const actual = read();
    if (
      now.supplier !== actual.supplier ||
      now.currency !== actual.currency ||
      now.date !== actual.date ||
      now.order !== actual.order
    ) {
      return;
    }
    prev.current = actual;
    if (entityKey === GOODS_RECEIPT && before.order !== order) {
      void applyOrder(order);
    } else if (before.supplier !== supplier && supplier) {
      void applySupplier(supplier);
    } else if (before.currency !== currency && currency) {
      void applyCurrency(currency);
    } else if (before.date !== date && date) {
      void applyDate(date);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [supplier, currency, date, order]);

  return null;
}

/** 進貨明細的批號：草稿可以輸入（留空時過帳自動產生）；過帳後連到該商品的庫存查詢。 */
export function ReceiptLotNoInput({
  row,
  label,
  disabled,
  value,
  onChange,
  id,
}: {
  row: (string | number)[];
  label: string;
  disabled?: boolean;
  value?: unknown;
  onChange?: (value: string) => void;
  id?: string;
}) {
  const form = Form.useFormInstance();
  const productId = textOf(Form.useWatch([...row, F.product], form));
  const text = typeof value === 'string' ? value : '';
  if (disabled && text && productId) {
    return (
      <Link to={productStockPath(productId)} title="查看此商品的批號與庫存">
        {text}
      </Link>
    );
  }
  return (
    <Input
      id={id}
      aria-label={label}
      disabled={disabled}
      value={text}
      placeholder={disabled ? undefined : '留空則過帳時自動產生'}
      style={{ minWidth: 180 }}
      onChange={(e) => onChange?.(e.target.value)}
    />
  );
}

/**
 * 進貨退回選了進貨單：帶入廠商、幣別、匯率、稅別（系統維護，顯示與金額預覽用）與可退數量大於 0 的明細
 * （§8.8「進貨退回」）。已有明細時先詢問是否取代。只有新增中的草稿使用（存檔後不可更換進貨單）。
 */
export function PurchaseReturnAutofill({
  details,
  onChange,
}: {
  details: readonly DetailTable[];
  onChange: () => void;
}) {
  const form = Form.useFormInstance();
  const queryClient = useQueryClient();
  const { message, modal } = App.useApp();
  const receiptId = textOf(Form.useWatch(F.goodsReceipt, form));
  const prev = useRef(textOf(form.getFieldValue(F.goodsReceipt)));

  const apply = async (id: string) => {
    const table = details.find((d) => d.entity.key === PURCHASE_RETURN_LINE);
    if (!table) {
      return;
    }
    const result = await queryClient
      .fetchQuery({ ...returnableQuery(id), staleTime: 0 })
      .catch(() => undefined);
    if (!result) {
      message.error('無法讀取進貨單');
      return;
    }
    const { receipt, returnable } = result;
    const reason = receiptNotReturnable(documentStatus(receipt));
    if (reason) {
      message.warning(`${reason}，無法退回`);
      return;
    }
    const lines = returnLinesFromReceipt(receipt, returnable);
    const name = receipt.record_no ?? '進貨單';
    const fill = () => {
      const rate = textOf(receipt.data[F.rate]);
      form.setFieldsValue({
        [F.supplier]: receipt.data[F.supplier] ?? undefined,
        [F.currency]: receipt.data[F.currency] ?? undefined,
        [F.rate]: rate ? trimRate(rate) : undefined,
        [F.taxType]: receipt.data[F.taxType] ?? undefined,
      });
      form.setFieldValue(
        [LINES_FORM_KEY, PURCHASE_RETURN_LINE],
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
    const rows = (form.getFieldValue([LINES_FORM_KEY, PURCHASE_RETURN_LINE]) ?? []) as unknown[];
    if (rows.length > 0) {
      modal.confirm({
        title: '要以進貨單的明細取代目前的明細嗎？',
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
    if (receiptId !== textOf(form.getFieldValue(F.goodsReceipt))) {
      return;
    }
    const before = prev.current;
    prev.current = receiptId;
    if (receiptId && receiptId !== before) {
      void apply(receiptId);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [receiptId]);

  return null;
}

/** 進貨退回明細的可退數量：進貨明細的數量減去其他已過帳退回的數量。 */
export function ReturnableQty({ row }: { row: (string | number)[] }) {
  const form = Form.useFormInstance();
  const receiptId = textOf(Form.useWatch(F.goodsReceipt, form));
  const lineId = textOf(Form.useWatch([...row, F.goodsReceiptLine], form));
  const result = useReturnable(receiptId);
  const qty = lineId ? result.data?.returnable.get(lineId) : undefined;
  return qty ? (
    <Typography.Text strong>{formatQty(qty)}</Typography.Text>
  ) : (
    <Typography.Text type="secondary">—</Typography.Text>
  );
}
