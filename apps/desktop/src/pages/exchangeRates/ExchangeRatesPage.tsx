import { PlusOutlined, ReloadOutlined, SearchOutlined } from '@ant-design/icons';
import {
  Alert,
  App,
  Button,
  DatePicker,
  Form,
  Popconfirm,
  Select,
  Space,
  Table,
  Tag,
  Tooltip,
} from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';

import { Page } from '../../components/PageHeader';
import { apiErrorCode, errorMessage } from '../../api/client';
import { useAllCurrencies } from '../../api/currencies';
import { useDeleteExchangeRate, useExchangeRates } from '../../api/exchangeRates';
import { usePermission } from '../../auth/permission';
import { PERMISSIONS } from '../../bindings';
import type { ExchangeRate } from '../../bindings';
import { ConflictAlert } from '../../components/ConflictAlert';
import {
  DEFAULT_RATE_PAGE_SIZE,
  buildExchangeRateQuery,
  rateFilterOptions,
} from '../../currencies/forms';
import type { ExchangeRateFilters } from '../../currencies/forms';
import { formatTaipei } from '../../utils/time';
import { ExchangeRateFormModal } from './ExchangeRateFormModal';

/**
 * 匯率管理頁（docs/architecture.md §6.1、§8.6）。
 *
 * 查看需要 `record.read`；新增、修改、刪除需要 `record.write`，沒有時不顯示這些按鈕
 * （只是輔助，權限仍由伺服器檢查）。
 */
export function ExchangeRatesPage() {
  const { message } = App.useApp();
  const canWrite = usePermission(PERMISSIONS.RECORD_WRITE);
  const [form] = Form.useForm<ExchangeRateFilters>();
  const [filters, setFilters] = useState<ExchangeRateFilters>({});
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(DEFAULT_RATE_PAGE_SIZE);
  const rates = useExchangeRates(buildExchangeRateQuery(filters, page, pageSize));
  const currencies = useAllCurrencies();
  const deleteRate = useDeleteExchangeRate();
  // `null` 為新增，`undefined` 為關閉
  const [editing, setEditing] = useState<ExchangeRate | null>();
  const [conflict, setConflict] = useState(false);

  const currencyByCode = new Map((currencies.data ?? []).map((c) => [c.code, c]));

  const search = (values: ExchangeRateFilters) => {
    setFilters(values);
    setPage(1);
  };

  const reload = () => {
    setConflict(false);
    void rates.refetch();
  };

  const remove = (rate: ExchangeRate) =>
    deleteRate.mutateAsync(rate).then(
      () => void message.success('已刪除匯率'),
      (err: unknown) => {
        if (apiErrorCode(err) === 'CONFLICT') {
          setConflict(true);
        } else {
          void message.error(errorMessage(err));
        }
      },
    );

  const columns: TableColumnsType<ExchangeRate> = [
    { title: '日期', dataIndex: 'rate_date', width: 120 },
    {
      title: '幣別',
      dataIndex: 'currency_code',
      render: (code: string) => {
        const c = currencyByCode.get(code);
        return (
          <Space size={4}>
            {c ? `${c.code} ${c.name}` : code}
            {c && !c.is_active && <Tag>已停用</Tag>}
          </Space>
        );
      },
    },
    { title: '匯率（新台幣）', dataIndex: 'rate', width: 160, align: 'right' },
    {
      title: '最後修改時間',
      dataIndex: 'updated_at',
      width: 180,
      render: (value: string) => formatTaipei(value),
    },
  ];
  if (canWrite) {
    columns.push({
      title: '操作',
      key: 'actions',
      width: 130,
      render: (_, rate) => {
        const inactive = currencyByCode.get(rate.currency_code)?.is_active === false;
        return (
          <Space size={0}>
            <Tooltip title={inactive ? '幣別已停用，不可修改匯率' : undefined}>
              <Button type="link" size="small" disabled={inactive} onClick={() => setEditing(rate)}>
                修改
              </Button>
            </Tooltip>
            <Popconfirm
              title={`確定要刪除 ${rate.currency_code} ${rate.rate_date} 的匯率？`}
              okText="刪除"
              okButtonProps={{ danger: true }}
              cancelText="取消"
              onConfirm={() => remove(rate)}
            >
              <Button type="link" size="small" danger>
                刪除
              </Button>
            </Popconfirm>
          </Space>
        );
      },
    });
  }

  return (
    <Page
      title="匯率"
      description="匯率為 1 單位外幣兌換的新台幣金額。"
      actions={
        <Space>
          <Button icon={<ReloadOutlined />} onClick={reload} loading={rates.isFetching}>
            重新整理
          </Button>
          {canWrite && (
            <Button type="primary" icon={<PlusOutlined />} onClick={() => setEditing(null)}>
              新增匯率
            </Button>
          )}
        </Space>
      }
    >
      <Form form={form} layout="inline" onFinish={search} style={{ marginBottom: 16, rowGap: 8 }}>
        <Form.Item label="幣別" name="currencyCode">
          <Select
            allowClear
            showSearch
            optionFilterProp="label"
            placeholder="全部"
            style={{ width: 200 }}
            loading={currencies.isPending}
            options={rateFilterOptions(currencies.data ?? [])}
          />
        </Form.Item>
        <Form.Item label="日期" name="dateRange">
          <DatePicker.RangePicker allowEmpty={[true, true]} />
        </Form.Item>
        <Form.Item>
          <Space>
            <Button type="primary" htmlType="submit" icon={<SearchOutlined />}>
              查詢
            </Button>
            <Button
              onClick={() => {
                form.resetFields();
                search({});
              }}
            >
              清除
            </Button>
          </Space>
        </Form.Item>
      </Form>
      {conflict && <ConflictAlert onReload={reload} loading={rates.isFetching} />}
      {rates.isError && (
        <Alert
          showIcon
          type="error"
          message={errorMessage(rates.error)}
          style={{ marginBottom: 16 }}
        />
      )}
      <Table<ExchangeRate>
        rowKey="id"
        size="small"
        columns={columns}
        dataSource={rates.data?.items}
        loading={rates.isFetching}
        pagination={{
          current: rates.data?.page ?? page,
          pageSize: rates.data?.page_size ?? pageSize,
          total: rates.data?.total ?? 0,
          showSizeChanger: true,
          pageSizeOptions: [20, 50, 100, 200],
          showTotal: (total) => `共 ${total} 筆`,
          onChange: (nextPage, nextSize) => {
            setPage(nextSize !== pageSize ? 1 : nextPage);
            setPageSize(nextSize);
          },
        }}
      />
      {canWrite && (
        <ExchangeRateFormModal
          open={editing !== undefined}
          rate={editing ?? null}
          currencies={currencies.data ?? []}
          onClose={() => setEditing(undefined)}
          onReload={reload}
        />
      )}
    </Page>
  );
}
