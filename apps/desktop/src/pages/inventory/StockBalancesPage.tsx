import { ReloadOutlined, WarningOutlined } from '@ant-design/icons';
import { Alert, Button, Card, Checkbox, Input, Space, Table, Tag, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';
import { useNavigate } from 'react-router';

import { errorMessage } from '../../api/client';
import { useStockBalances } from '../../api/inventory';
import type { StockBalance_Serialize as StockBalance } from '../../bindings';
import { formatQty } from '../../records/documents';
import { formatAmount, productStockPath } from '../../records/inventory';

const DEFAULT_PAGE_SIZE = 50;

/** 低於安全庫存的標示。 */
export const BELOW_SAFETY_STOCK_TEXT = '低於安全庫存';

const columns: TableColumnsType<StockBalance> = [
  { title: '料號', dataIndex: 'code', width: 140 },
  {
    title: '品名',
    dataIndex: 'name',
    render: (name: string, row) => (
      <Space size={4}>
        {name}
        {!row.is_active && <Tag>停用</Tag>}
      </Space>
    ),
  },
  {
    title: '規格',
    dataIndex: 'spec',
    render: (spec: string | null) => spec ?? <Typography.Text type="secondary">—</Typography.Text>,
  },
  { title: '單位', dataIndex: 'unit_name', width: 80 },
  {
    title: '庫存數量',
    dataIndex: 'qty_on_hand',
    align: 'right',
    width: 120,
    render: (qty: string, row) => (
      <Typography.Text type={row.below_safety_stock ? 'danger' : undefined} strong>
        {formatQty(qty)}
      </Typography.Text>
    ),
  },
  {
    title: '安全庫存',
    dataIndex: 'safety_stock',
    align: 'right',
    width: 120,
    render: (qty: string | null) =>
      qty === null ? <Typography.Text type="secondary">—</Typography.Text> : formatQty(qty),
  },
  {
    title: '庫存金額',
    dataIndex: 'stock_value_twd',
    align: 'right',
    width: 140,
    render: (value: string) => formatAmount(value),
  },
  {
    title: '狀態',
    key: 'status',
    width: 140,
    render: (_, row) =>
      row.below_safety_stock ? (
        <Tag color="red" icon={<WarningOutlined />}>
          {BELOW_SAFETY_STOCK_TEXT}
        </Tag>
      ) : null,
  },
];

/**
 * 庫存查詢（`/stock`，需要 `record.read`）：商品庫存列表，依料號排序；低於安全庫存的商品以紅色標示。
 * 點商品開啟批號與異動流水帳。
 */
export function StockBalancesPage() {
  const navigate = useNavigate();
  const [q, setQ] = useState('');
  const [inStock, setInStock] = useState(false);
  const [belowSafety, setBelowSafety] = useState(false);
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(DEFAULT_PAGE_SIZE);

  const balances = useStockBalances({
    q: q || null,
    in_stock: inStock ? 'true' : null,
    below_safety_stock: belowSafety ? 'true' : null,
    page,
    page_size: pageSize,
  });

  return (
    <Card
      title="庫存查詢"
      extra={
        <Button
          icon={<ReloadOutlined />}
          onClick={() => balances.refetch()}
          loading={balances.isFetching}
        >
          重新整理
        </Button>
      }
    >
      <Space wrap style={{ marginBottom: 16 }}>
        <Input.Search
          aria-label="料號或品名"
          placeholder="料號或品名"
          allowClear
          style={{ width: 260 }}
          onSearch={(value) => {
            setQ(value.trim());
            setPage(1);
          }}
        />
        <Checkbox
          checked={inStock}
          onChange={(e) => {
            setInStock(e.target.checked);
            setPage(1);
          }}
        >
          只顯示有庫存
        </Checkbox>
        <Checkbox
          checked={belowSafety}
          onChange={(e) => {
            setBelowSafety(e.target.checked);
            setPage(1);
          }}
        >
          只顯示{BELOW_SAFETY_STOCK_TEXT}
        </Checkbox>
      </Space>
      {balances.isError && (
        <Alert
          showIcon
          type="error"
          message={errorMessage(balances.error)}
          style={{ marginBottom: 16 }}
        />
      )}
      <Table<StockBalance>
        rowKey="product_id"
        size="small"
        columns={columns}
        dataSource={balances.data?.items}
        loading={balances.isFetching}
        scroll={{ x: 'max-content' }}
        onRow={(row) => ({
          onClick: () => navigate(productStockPath(row.product_id)),
          style: { cursor: 'pointer' },
        })}
        locale={{ emptyText: '沒有商品' }}
        pagination={{
          current: balances.data?.page ?? page,
          pageSize: balances.data?.page_size ?? pageSize,
          total: balances.data?.total ?? 0,
          showSizeChanger: true,
          pageSizeOptions: [20, 50, 100, 200],
          showTotal: (total) => `共 ${total} 筆`,
          onChange: (nextPage, nextSize) => {
            setPage(nextSize !== pageSize ? 1 : nextPage);
            setPageSize(nextSize);
          },
        }}
      />
    </Card>
  );
}
