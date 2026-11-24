import { PlusOutlined, ReloadOutlined } from '@ant-design/icons';
import { Alert, App, Button, Popconfirm, Space, Table, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';

import { StatusTag } from '../../components/StatusTag';
import { Page } from '../../components/PageHeader';
import { apiErrorCode, errorMessage } from '../../api/client';
import { useAllCurrencies, useUpdateCurrency } from '../../api/currencies';
import type { Currency } from '../../bindings';
import { ConflictAlert } from '../../components/ConflictAlert';
import { CurrencyFormModal } from './CurrencyFormModal';

/** 幣別管理頁（需要 `settings.manage`，docs/architecture.md §6.1、§8.6）。幣別不可刪除，只能停用。 */
export function CurrenciesPage() {
  const { message } = App.useApp();
  const currencies = useAllCurrencies();
  const updateCurrency = useUpdateCurrency();
  // `null` 為新增，`undefined` 為關閉
  const [editing, setEditing] = useState<Currency | null>();
  const [conflict, setConflict] = useState(false);

  const reload = () => {
    setConflict(false);
    void currencies.refetch();
  };

  const toggleActive = (currency: Currency) =>
    updateCurrency
      .mutateAsync({
        code: currency.code,
        req: {
          version: currency.version,
          name: null,
          symbol: null,
          decimal_places: null,
          is_active: !currency.is_active,
        },
      })
      .then(
        () =>
          void message.success(`已${currency.is_active ? '停用' : '啟用'}幣別 ${currency.code}`),
        (err: unknown) => {
          if (apiErrorCode(err) === 'CONFLICT') {
            setConflict(true);
          } else {
            void message.error(errorMessage(err));
          }
        },
      );

  const columns: TableColumnsType<Currency> = [
    { title: '代碼', dataIndex: 'code', width: 90 },
    { title: '名稱', dataIndex: 'name' },
    {
      title: '符號',
      dataIndex: 'symbol',
      width: 90,
      render: (value: string | null) =>
        value ?? <Typography.Text type="secondary">—</Typography.Text>,
    },
    { title: '小數位數', dataIndex: 'decimal_places', width: 100, align: 'right' },
    {
      title: '狀態',
      key: 'status',
      width: 160,
      render: (_, c) => (
        <Space size={4}>
          {c.is_base && <StatusTag tone="info">本位幣</StatusTag>}
          {c.is_active ? (
            <StatusTag tone="success">啟用</StatusTag>
          ) : (
            <StatusTag tone="neutral">停用</StatusTag>
          )}
        </Space>
      ),
    },
    {
      title: '操作',
      key: 'actions',
      width: 140,
      render: (_, c) => (
        <Space size={0}>
          <Button type="link" size="small" onClick={() => setEditing(c)}>
            編輯
          </Button>
          {!c.is_base && (
            <Popconfirm
              title={`確定要${c.is_active ? '停用' : '啟用'}幣別 ${c.code}？`}
              description={
                c.is_active
                  ? '停用後不出現在幣別選單，也不可新增或修改它的匯率；已存的資料保留。'
                  : undefined
              }
              okText={c.is_active ? '停用' : '啟用'}
              okButtonProps={{ danger: c.is_active }}
              cancelText="取消"
              onConfirm={() => toggleActive(c)}
            >
              <Button type="link" size="small" danger={c.is_active}>
                {c.is_active ? '停用' : '啟用'}
              </Button>
            </Popconfirm>
          )}
        </Space>
      ),
    },
  ];

  return (
    <Page
      title="幣別"
      actions={
        <Space>
          <Button icon={<ReloadOutlined />} onClick={reload} loading={currencies.isFetching}>
            重新整理
          </Button>
          <Button type="primary" icon={<PlusOutlined />} onClick={() => setEditing(null)}>
            新增幣別
          </Button>
        </Space>
      }
    >
      {conflict && <ConflictAlert onReload={reload} loading={currencies.isFetching} />}
      {currencies.isError && (
        <Alert
          showIcon
          type="error"
          message={errorMessage(currencies.error)}
          style={{ marginBottom: 16 }}
        />
      )}
      <Table<Currency>
        rowKey="id"
        size="small"
        columns={columns}
        dataSource={currencies.data}
        loading={currencies.isPending}
        pagination={false}
      />
      <CurrencyFormModal
        open={editing !== undefined}
        currency={editing ?? null}
        onClose={() => setEditing(undefined)}
        onReload={reload}
      />
    </Page>
  );
}
