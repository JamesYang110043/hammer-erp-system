import { Select } from 'antd';

import { useActiveCurrencies } from '../../api/currencies';
import { currencyOptions } from '../../records/masterData';

/**
 * 幣別選單（`ui_widget = currency`，例如廠商的預設幣別）：只列出啟用中的幣別，值為幣別代碼。
 *
 * 既有資料的幣別已停用時仍顯示原值（標示「已停用」），可以保留或改選其他幣別（§7.2）。
 */
export function CurrencySelect({
  value,
  onChange,
  disabled,
  id,
  'aria-label': ariaLabel,
}: {
  value?: string;
  onChange?: (value: string | undefined) => void;
  disabled?: boolean;
  id?: string;
  'aria-label'?: string;
}) {
  const currencies = useActiveCurrencies();
  return (
    <Select
      id={id}
      aria-label={ariaLabel}
      disabled={disabled}
      style={{ maxWidth: 320, width: '100%' }}
      showSearch
      optionFilterProp="label"
      loading={currencies.isPending}
      status={currencies.isError ? 'error' : undefined}
      placeholder={currencies.isError ? '無法載入幣別' : '選擇幣別'}
      value={value || undefined}
      options={currencyOptions(currencies.data ?? [], value)}
      onChange={(v: string | undefined) => onChange?.(v)}
    />
  );
}
