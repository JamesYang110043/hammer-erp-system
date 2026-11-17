import { Alert, App, Form, Input, InputNumber, Modal, Switch } from 'antd';
import { useState } from 'react';

import { apiErrorCode } from '../../api/client';
import { useCreateCurrency, useUpdateCurrency } from '../../api/currencies';
import type { Currency } from '../../bindings';
import { ConflictAlert } from '../../components/ConflictAlert';
import {
  CURRENCY_CODE_RE,
  MAX_CURRENCY_DECIMAL_PLACES,
  buildCreateCurrency,
  buildUpdateCurrency,
} from '../../currencies/forms';
import type { CurrencyFormValues } from '../../currencies/forms';
import { applyFormError } from '../users/formErrors';

/**
 * 新增或編輯幣別（需要 `settings.manage`）。
 *
 * - `currency` 為 `null` 時新增；有值時編輯（代碼不可修改，只送出有變更的欄位並帶上版本號）
 * - 本位幣不可停用
 * - 版本衝突時提示重新載入：重新取得列表並關閉視窗
 */
export function CurrencyFormModal({
  open,
  currency,
  onClose,
  onReload,
}: {
  open: boolean;
  currency: Currency | null;
  onClose: () => void;
  onReload: () => void;
}) {
  const { message } = App.useApp();
  const [form] = Form.useForm<CurrencyFormValues>();
  const createCurrency = useCreateCurrency();
  const updateCurrency = useUpdateCurrency();
  const [error, setError] = useState<string>();
  const [conflict, setConflict] = useState(false);
  const pending = createCurrency.isPending || updateCurrency.isPending;

  const close = () => {
    setError(undefined);
    setConflict(false);
    createCurrency.reset();
    updateCurrency.reset();
    onClose();
  };

  const onError = (err: unknown) => {
    if (apiErrorCode(err) === 'CONFLICT') {
      setConflict(true);
      return;
    }
    setError(applyFormError(form, err));
  };

  const submit = (values: CurrencyFormValues) => {
    setError(undefined);
    if (!currency) {
      createCurrency.mutate(buildCreateCurrency(values), {
        onSuccess: (saved) => {
          message.success(`已新增幣別 ${saved.code}`);
          close();
        },
        onError,
      });
      return;
    }
    const req = buildUpdateCurrency(currency, values);
    if (!req) {
      message.info('沒有需要儲存的修改');
      close();
      return;
    }
    updateCurrency.mutate(
      { code: currency.code, req },
      {
        onSuccess: () => {
          message.success(`已儲存幣別 ${currency.code}`);
          close();
        },
        onError,
      },
    );
  };

  return (
    <Modal
      title={currency ? `編輯幣別：${currency.code}` : '新增幣別'}
      open={open}
      onCancel={close}
      onOk={() => form.submit()}
      okText="儲存"
      okButtonProps={{ loading: pending, disabled: conflict }}
      destroyOnHidden
    >
      {conflict && (
        <ConflictAlert
          description="重新載入會取得最新的幣別資料並關閉此視窗，請再重新修改。"
          onReload={() => {
            onReload();
            close();
          }}
        />
      )}
      <Form<CurrencyFormValues>
        form={form}
        layout="vertical"
        onFinish={submit}
        disabled={pending || conflict}
        preserve={false}
        initialValues={
          currency
            ? {
                code: currency.code,
                name: currency.name,
                symbol: currency.symbol ?? '',
                decimal_places: currency.decimal_places,
                is_active: currency.is_active,
              }
            : { decimal_places: 2, is_active: true }
        }
      >
        <Form.Item
          label="代碼"
          name="code"
          normalize={(v: string | undefined) => v?.toUpperCase()}
          extra={currency ? '代碼建立後不可修改' : 'ISO 4217 代碼，例如 GBP'}
          rules={
            currency
              ? []
              : [
                  { required: true, message: '請輸入幣別代碼' },
                  { pattern: CURRENCY_CODE_RE, message: '幣別代碼必須是 3 個英文字母' },
                ]
          }
        >
          <Input autoComplete="off" maxLength={3} disabled={currency !== null} />
        </Form.Item>
        <Form.Item
          label="名稱"
          name="name"
          rules={[{ required: true, whitespace: true, message: '請輸入幣別名稱' }]}
        >
          <Input autoComplete="off" />
        </Form.Item>
        <Form.Item label="符號" name="symbol">
          <Input autoComplete="off" placeholder="例如 £" />
        </Form.Item>
        <Form.Item
          label="小數位數"
          name="decimal_places"
          extra="金額的小數位數；修改後只影響之後輸入的金額"
          rules={[{ required: true, message: '請輸入小數位數' }]}
        >
          <InputNumber min={0} max={MAX_CURRENCY_DECIMAL_PLACES} precision={0} />
        </Form.Item>
        {currency && (
          <Form.Item
            label="啟用"
            name="is_active"
            valuePropName="checked"
            extra={
              currency.is_base
                ? '本位幣不可停用'
                : '停用後不出現在幣別選單，也不可新增或修改它的匯率；已存的資料保留'
            }
          >
            <Switch disabled={currency.is_base} />
          </Form.Item>
        )}
        {error && <Alert showIcon type="error" message={error} />}
      </Form>
    </Modal>
  );
}
