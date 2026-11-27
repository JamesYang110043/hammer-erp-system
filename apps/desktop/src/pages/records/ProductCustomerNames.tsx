import { Alert, Table, Typography } from 'antd';
import type { TableColumnsType } from 'antd';
import { Link } from 'react-router';

import { errorMessage } from '../../api/client';
import { useProductCustomerNames } from '../../api/sales';
import type { ProductCustomerName } from '../../bindings';
import { StatusTag } from '../../components/StatusTag';
import { SectionTitle } from '../../components/SectionTitle';
import { splitViewPath } from '../../records/masterSplit';

/** 沒有客戶品名時的文字。 */
export const NO_CUSTOMER_PRODUCT_NAMES = '尚未設定客戶品名';

/**
 * 商品畫面的客戶品名（docs/architecture.md §8.7「商品畫面的客戶品名」，M12）：唯讀表格，列出客戶、
 * 客戶品名、備註（依客戶編號排序）。固定顯示在表單最後，不納入表單版面。點客戶開啟客戶畫面並選取該客戶；
 * 維護仍在客戶畫面。
 */
export function ProductCustomerNames({ productId }: { productId: string }) {
  const names = useProductCustomerNames(productId);
  const columns: TableColumnsType<ProductCustomerName> = [
    {
      key: 'customer',
      title: '客戶',
      render: (_, row) => (
        <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4 }}>
          <Link to={splitViewPath('customer', { selected: row.customer_id })}>
            {row.customer_title}
          </Link>
          {!row.customer_is_active && (
            <StatusTag tone="neutral" style={{ marginInlineEnd: 0 }}>
              停用
            </StatusTag>
          )}
        </span>
      ),
    },
    { key: 'name', title: '客戶品名', dataIndex: 'name' },
    {
      key: 'notes',
      title: '備註',
      render: (_, row) =>
        row.notes ? (
          <Typography.Text style={{ whiteSpace: 'pre-wrap' }}>{row.notes}</Typography.Text>
        ) : (
          <Typography.Text type="secondary">—</Typography.Text>
        ),
    },
  ];
  return (
    <section aria-label="客戶品名" style={{ marginTop: 8, marginBottom: 16 }}>
      <SectionTitle>客戶品名</SectionTitle>
      {names.isError ? (
        <Alert showIcon type="error" message={`無法讀取客戶品名：${errorMessage(names.error)}`} />
      ) : (
        <Table<ProductCustomerName>
          rowKey="id"
          size="small"
          columns={columns}
          dataSource={names.data ?? []}
          loading={names.isLoading}
          pagination={false}
          scroll={{ x: 'max-content' }}
          locale={{ emptyText: NO_CUSTOMER_PRODUCT_NAMES }}
        />
      )}
    </section>
  );
}
