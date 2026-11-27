import { Table, Typography } from 'antd';
import { useState } from 'react';

import type { CustomerNameSuggestion } from '../../records/sales';

/** 對話框的表格：每列可勾選（預設全勾）。勾選變動時呼叫 `onChange`。 */
export function CustomerNameSuggestionTable({
  rows,
  onChange,
}: {
  rows: CustomerNameSuggestion[];
  onChange: (selected: string[]) => void;
}) {
  const [selected, setSelected] = useState<string[]>(() => rows.map((r) => r.product_id));
  return (
    <Table<CustomerNameSuggestion>
      rowKey="product_id"
      size="small"
      pagination={false}
      dataSource={rows}
      scroll={{ x: 'max-content', y: 360 }}
      rowSelection={{
        selectedRowKeys: selected,
        onChange: (keys) => {
          const next = keys.map(String);
          setSelected(next);
          onChange(next);
        },
      }}
      columns={[
        { key: 'product', title: '商品', dataIndex: 'product_title' },
        {
          key: 'current',
          title: '目前的品名',
          render: (_, r) => <Typography.Text type="secondary">{r.current_name}</Typography.Text>,
        },
        { key: 'document', title: '單據上的品名', dataIndex: 'document_name' },
      ]}
    />
  );
}
