import type { FormInstance } from 'antd';
import { useEffect } from 'react';

import { useAllCurrencies, useTaxRates } from '../../api/currencies';
import { LINES_FORM_KEY } from '../../records/lines';
import type { LineFormValues } from '../../records/lines';
import {
  PURCHASING_FIELDS as F,
  documentTotals,
  lineAmount,
  amountLineKey,
} from '../../records/purchasing';
import { SALES_FIELDS as S, salesLineAmount, salesLineKey, salesTotals } from '../../records/sales';
import {
  DUPLICATE,
  INVOICING_FIELDS as I,
  SALES_INVOICE,
  invoiceLineAmount,
  invoiceTotals,
  invoicingLineKey,
  isInvoicingDocument,
  dateText,
  enteredTotal,
  hasEnteredAmounts,
  taxPeriod,
} from '../../records/invoicing';

/**
 * 明細金額與合計的即時預覽：依表單目前的值（`values`）計算，寫入唯讀的金額欄位
 * （唯讀欄位不送出、不算修改，存檔時以伺服器計算為準，§8.8、§8.9「金額與稅額」）。
 *
 * 幣別的小數位數或稅率還沒載入、或幣別、稅別沒有值時不計算。
 */
export function useAmountPreview(
  form: FormInstance,
  entityKey: string,
  values: Record<string, unknown>,
  enabled: boolean,
) {
  const salesKey = salesLineKey(entityKey);
  const invoiceKey = invoicingLineKey(entityKey);
  const lineKey = amountLineKey(entityKey) ?? salesKey ?? invoiceKey;
  const invoicing = isInvoicingDocument(entityKey);
  // 只有採購單、進貨單、進貨退回、訂單、出貨單的草稿需要
  const currencies = useAllCurrencies(enabled && lineKey !== undefined);
  const taxRates = useTaxRates(enabled && lineKey !== undefined);
  useEffect(() => {
    if (!enabled) {
      return;
    }
    // 發票模組：期別由發票（折讓）日期計算（§8.11「期別」）
    if (invoicing) {
      const date = values[I.invoiceDate] ?? values[I.allowanceDate];
      form.setFieldValue(I.taxPeriod, taxPeriod(dateText(date)));
    }
    // 進項憑證、進貨折讓沒有明細：總計 = 輸入的未稅金額 + 稅額
    if (hasEnteredAmounts(entityKey)) {
      form.setFieldValue(I.total, enteredTotal(values[I.subtotal], values[I.taxAmount]));
      return;
    }
    if (!lineKey) {
      return;
    }
    if (invoiceKey) {
      // 發票、折讓以台幣計算；三聯式、折讓單價未稅，二聯式含稅；手動稅額 ±1 元（§8.11「金額與稅額」）
      const twd =
        currencies.data?.find((c) => c.code === 'TWD') ?? currencies.data?.find((c) => c.is_base);
      const tax = taxRates.data?.find((t) => t.code === values[I.taxType]);
      const lines = ((values[LINES_FORM_KEY] as Record<string, LineFormValues[]> | undefined)?.[
        invoiceKey
      ] ?? []) as LineFormValues[];
      const amounts = lines.map((line) =>
        twd ? invoiceLineAmount(line[I.qty], line[I.unitPrice], twd.decimal_places) : undefined,
      );
      amounts.forEach((amount, i) => {
        form.setFieldValue([LINES_FORM_KEY, invoiceKey, i, I.amount], amount);
      });
      const totals =
        twd && tax
          ? invoiceTotals(
              amounts.filter((a): a is string => a !== undefined),
              tax.rate,
              entityKey === SALES_INVOICE && values[I.invoiceType] === DUPLICATE,
              twd.decimal_places,
              values[I.taxManual],
            )
          : undefined;
      form.setFieldsValue({
        [I.subtotal]: totals?.subtotal,
        [I.taxAmount]: totals?.tax_amount,
        [I.total]: totals?.total,
      });
      return;
    }
    if (salesKey) {
      // 銷貨以台幣交易（§8.9）：小數位數取台幣（本位幣）
      const twd =
        currencies.data?.find((c) => c.code === 'TWD') ?? currencies.data?.find((c) => c.is_base);
      const tax = taxRates.data?.find((t) => t.code === values[S.taxType]);
      const lines = ((values[LINES_FORM_KEY] as Record<string, LineFormValues[]> | undefined)?.[
        salesKey
      ] ?? []) as LineFormValues[];
      const amounts = lines.map((line) =>
        twd
          ? salesLineAmount(
              line[S.qty],
              line[S.unitPrice],
              line[S.discountRate],
              twd.decimal_places,
            )
          : undefined,
      );
      amounts.forEach((amount, i) => {
        form.setFieldValue([LINES_FORM_KEY, salesKey, i, S.amount], amount);
      });
      const totals =
        twd && tax
          ? salesTotals(
              amounts.filter((a): a is string => a !== undefined),
              tax.rate,
              values[S.taxIncluded] === true,
              twd.decimal_places,
            )
          : undefined;
      form.setFieldsValue({
        [S.subtotal]: totals?.subtotal,
        [S.taxAmount]: totals?.tax_amount,
        [S.total]: totals?.total,
      });
      return;
    }
    const code = values[F.currency];
    const currency = currencies.data?.find((c) => c.code === code);
    const tax = taxRates.data?.find((t) => t.code === values[F.taxType]);
    const lines = ((values[LINES_FORM_KEY] as Record<string, LineFormValues[]> | undefined)?.[
      lineKey
    ] ?? []) as LineFormValues[];
    const amounts = lines.map((line) =>
      currency ? lineAmount(line[F.qty], line[F.unitPrice], currency.decimal_places) : undefined,
    );
    amounts.forEach((amount, i) => {
      form.setFieldValue([LINES_FORM_KEY, lineKey, i, F.amount], amount);
    });
    if (!currency || !tax) {
      form.setFieldsValue({
        [F.subtotal]: undefined,
        [F.taxAmount]: undefined,
        [F.total]: undefined,
        [F.totalTwd]: undefined,
      });
      return;
    }
    const totals = documentTotals(
      amounts.filter((a): a is string => a !== undefined),
      tax.rate,
      currency.decimal_places,
      values[F.rate],
    );
    form.setFieldsValue({
      [F.subtotal]: totals.subtotal,
      [F.taxAmount]: totals.tax_amount,
      [F.total]: totals.total,
      [F.totalTwd]: totals.total_twd ?? undefined,
    });
  }, [
    form,
    entityKey,
    lineKey,
    salesKey,
    invoiceKey,
    invoicing,
    enabled,
    values,
    currencies.data,
    taxRates.data,
  ]);
}
