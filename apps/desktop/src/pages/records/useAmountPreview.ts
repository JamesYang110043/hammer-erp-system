import type { FormInstance } from 'antd';
import { useEffect } from 'react';

import { useAllCurrencies, useTaxRates } from '../../api/currencies';
import { LINES_FORM_KEY } from '../../records/lines';
import type { LineFormValues } from '../../records/lines';
import {
  PURCHASING_FIELDS as F,
  documentTotals,
  lineAmount,
  pricedDocument,
} from '../../records/purchasing';

/**
 * 明細金額與合計的即時預覽：依表單目前的值（`values`）計算，寫入唯讀的金額欄位
 * （唯讀欄位不送出、不算修改，存檔時以伺服器計算為準，§8.8「金額與稅額」）。
 *
 * 幣別的小數位數或稅率還沒載入、或幣別、稅別沒有值時不計算。
 */
export function useAmountPreview(
  form: FormInstance,
  entityKey: string,
  values: Record<string, unknown>,
  enabled: boolean,
) {
  const doc = pricedDocument(entityKey);
  // 只有採購單、進貨單的草稿需要
  const currencies = useAllCurrencies(enabled && doc !== undefined);
  const taxRates = useTaxRates(enabled && doc !== undefined);
  useEffect(() => {
    if (!enabled || !doc) {
      return;
    }
    const code = values[F.currency];
    const currency = currencies.data?.find((c) => c.code === code);
    const tax = taxRates.data?.find((t) => t.code === values[F.taxType]);
    const lines = ((values[LINES_FORM_KEY] as Record<string, LineFormValues[]> | undefined)?.[
      doc.lineKey
    ] ?? []) as LineFormValues[];
    const amounts = lines.map((line) =>
      currency ? lineAmount(line[F.qty], line[F.unitPrice], currency.decimal_places) : undefined,
    );
    amounts.forEach((amount, i) => {
      form.setFieldValue([LINES_FORM_KEY, doc.lineKey, i, F.amount], amount);
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
  }, [form, doc, enabled, values, currencies.data, taxRates.data]);
}
