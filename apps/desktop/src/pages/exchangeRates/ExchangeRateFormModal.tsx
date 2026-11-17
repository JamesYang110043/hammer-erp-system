import { Alert, App, DatePicker, Form, Input, Modal, Select } from 'antd';
import dayjs from 'dayjs';
import type { Dayjs } from 'dayjs';
import { useState } from 'react';

import { apiErrorCode } from '../../api/client';
import { useCreateExchangeRate, useUpdateExchangeRate } from '../../api/exchangeRates';
import type { Currency, ExchangeRate } from '../../bindings';
import { ConflictAlert } from '../../components/ConflictAlert';
import {
  currencyLabel,
  rateCurrencyOptions,
  sameRate,
  validateRateText,
} from '../../currencies/forms';
import { taipeiToday } from '../../utils/time';
import { applyFormError } from '../users/formErrors';

interface FormValues {
  currency_code?: string;
  rate_date?: Dayjs | null;
  rate?: string;
}

/**
 * 新增或修改匯率（需要 `record.write`）。
 *
 * - `rate` 為 `null` 時新增：選擇幣別（啟用中的外幣）、日期（預設台北時間今天）、匯率
 * - 有值時只能修改匯率；要換日期或幣別時刪除後重新新增
 * - 匯率以字串送出，不轉成浮點數；伺服器的欄位錯誤（例如同一天重複）顯示在欄位下方
 * - 版本衝突時提示重新載入：重新取得列表並關閉視窗
 */
export function ExchangeRateFormModal({
  open,
  rate,
  currencies,
  onClose,
  onReload,
}: {
  open: boolean;
  rate: ExchangeRate | null;
  currencies: readonly Currency[];
  onClose: () => void;
  onReload: () => void;
}) {
  const { message } = App.useApp();
  const [form] = Form.useForm<FormValues>();
  const createRate = useCreateExchangeRate();
  const updateRate = useUpdateExchangeRate();
  const [error, setError] = useState<string>();
  const [conflict, setConflict] = useState(false);
  const pending = createRate.isPending || updateRate.isPending;

  const close = () => {
    setError(undefined);
    setConflict(false);
    createRate.reset();
    updateRate.reset();
    onClose();
  };

  const onError = (err: unknown) => {
    if (apiErrorCode(err) === 'CONFLICT') {
      setConflict(true);
      return;
    }
    setError(applyFormError(form, err));
  };

  const submit = (values: FormValues) => {
    setError(undefined);
    const text = (values.rate ?? '').trim();
    if (!rate) {
      createRate.mutate(
        {
          currency_code: values.currency_code ?? '',
          rate_date: values.rate_date?.format('YYYY-MM-DD') ?? '',
          rate: text,
        },
        {
          onSuccess: () => {
            message.success('已新增匯率');
            close();
          },
          onError,
        },
      );
      return;
    }
    if (sameRate(text, rate.rate)) {
      message.info('沒有需要儲存的修改');
      close();
      return;
    }
    updateRate.mutate(
      { id: rate.id, req: { version: rate.version, rate: text } },
      {
        onSuccess: () => {
          message.success('已儲存匯率');
          close();
        },
        onError,
      },
    );
  };

  // 編輯時顯示原本的幣別（可能已停用，不在可選的清單中）
  const current = rate ? currencies.find((c) => c.code === rate.currency_code) : undefined;
  const currencyOptions = rate
    ? [{ value: rate.currency_code, label: current ? currencyLabel(current) : rate.currency_code }]
    : rateCurrencyOptions(currencies);

  return (
    <Modal
      title={rate ? '修改匯率' : '新增匯率'}
      open={open}
      onCancel={close}
      onOk={() => form.submit()}
      okText="儲存"
      okButtonProps={{ loading: pending, disabled: conflict }}
      destroyOnHidden
    >
      {conflict && (
        <ConflictAlert
          description="重新載入會取得最新的匯率並關閉此視窗，請再重新修改。"
          onReload={() => {
            onReload();
            close();
          }}
        />
      )}
      <Form<FormValues>
        form={form}
        layout="vertical"
        onFinish={submit}
        disabled={pending || conflict}
        preserve={false}
        initialValues={
          rate
            ? {
                currency_code: rate.currency_code,
                rate_date: dayjs(rate.rate_date),
                rate: rate.rate,
              }
            : { rate_date: dayjs(taipeiToday()) }
        }
      >
        <Form.Item
          label="幣別"
          name="currency_code"
          rules={[{ required: true, message: '請選擇幣別' }]}
        >
          <Select
            showSearch
            optionFilterProp="label"
            placeholder="選擇幣別"
            options={currencyOptions}
            disabled={rate !== null}
          />
        </Form.Item>
        <Form.Item
          label="日期"
          name="rate_date"
          extra={rate ? '要變更幣別或日期時，請刪除後重新新增' : undefined}
          rules={[{ required: true, message: '請選擇日期' }]}
        >
          <DatePicker allowClear={false} disabled={rate !== null} />
        </Form.Item>
        <Form.Item
          label="匯率"
          name="rate"
          extra="1 單位外幣兌換的新台幣金額，最多 6 位小數"
          rules={[
            {
              validator: (_, value: string | undefined) => {
                const msg = validateRateText(value);
                return msg ? Promise.reject(new Error(msg)) : Promise.resolve();
              },
            },
          ]}
        >
          <Input autoComplete="off" inputMode="decimal" placeholder="例如 32.15" />
        </Form.Item>
        {error && <Alert showIcon type="error" message={error} />}
      </Form>
    </Modal>
  );
}
