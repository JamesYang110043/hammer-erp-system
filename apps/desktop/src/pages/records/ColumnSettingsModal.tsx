import { ArrowDownOutlined, ArrowUpOutlined } from '@ant-design/icons';
import { Alert, Button, Checkbox, InputNumber, Modal, Space, Table } from 'antd';
import type { TableColumnsType } from 'antd';
import { useState } from 'react';

import type { FieldDefinition } from '../../bindings';
import { columnSettings, moveSetting, settingsToColumns } from '../../records/columns';
import type { ColumnSetting, LayoutColumn, LayoutConfig } from '../../records/columns';

/** 欄寬的範圍（px）。伺服器只要求大於 0。 */
const MIN_WIDTH = 40;
const MAX_WIDTH = 2000;

function ColumnSettingsForm({
  fields,
  config,
  onSubmit,
  onCancel,
}: {
  fields: readonly FieldDefinition[];
  config: LayoutConfig;
  onSubmit: (columns: LayoutColumn[]) => void;
  onCancel: () => void;
}) {
  const [items, setItems] = useState<ColumnSetting[]>(() => columnSettings(fields, config));
  const defaultWidths = new Map(fields.map((f) => [f.field_key, f.column_width]));
  const noneVisible = !items.some((i) => i.visible);

  const update = (index: number, patch: Partial<ColumnSetting>) =>
    setItems((prev) => prev.map((item, i) => (i === index ? { ...item, ...patch } : item)));

  const columns: TableColumnsType<ColumnSetting> = [
    {
      title: '顯示',
      key: 'visible',
      width: 60,
      render: (_, item, index) => (
        <Checkbox
          aria-label={`顯示${item.label}`}
          checked={item.visible}
          onChange={(e) => update(index, { visible: e.target.checked })}
        />
      ),
    },
    { title: '欄位', dataIndex: 'label' },
    {
      title: '欄寬（px）',
      key: 'width',
      width: 150,
      render: (_, item, index) => (
        <InputNumber<number>
          aria-label={`${item.label}的欄寬`}
          min={MIN_WIDTH}
          max={MAX_WIDTH}
          precision={0}
          placeholder={String(defaultWidths.get(item.fieldKey) ?? '自動')}
          value={item.width}
          onChange={(width) => update(index, { width })}
        />
      ),
    },
    {
      title: '順序',
      key: 'order',
      width: 90,
      render: (_, item, index) => (
        <Space size={0}>
          <Button
            type="text"
            size="small"
            aria-label={`${item.label}上移`}
            icon={<ArrowUpOutlined />}
            disabled={index === 0}
            onClick={() => setItems((prev) => moveSetting(prev, index, -1))}
          />
          <Button
            type="text"
            size="small"
            aria-label={`${item.label}下移`}
            icon={<ArrowDownOutlined />}
            disabled={index === items.length - 1}
            onClick={() => setItems((prev) => moveSetting(prev, index, 1))}
          />
        </Space>
      ),
    },
  ];

  return (
    <>
      {noneVisible && (
        <Alert showIcon type="warning" message="至少要顯示一個欄位" style={{ marginBottom: 12 }} />
      )}
      <Table<ColumnSetting>
        rowKey="fieldKey"
        size="small"
        columns={columns}
        dataSource={items}
        pagination={false}
      />
      <Space style={{ marginTop: 16, display: 'flex', justifyContent: 'flex-end' }}>
        <Button onClick={onCancel}>取消</Button>
        <Button
          type="primary"
          disabled={noneVisible}
          onClick={() => onSubmit(settingsToColumns(items))}
        >
          套用
        </Button>
      </Space>
    </>
  );
}

/** 調整顯示的欄位、順序與欄寬。 */
export function ColumnSettingsModal({
  open,
  fields,
  config,
  onSubmit,
  onClose,
}: {
  open: boolean;
  fields: readonly FieldDefinition[];
  config: LayoutConfig;
  onSubmit: (columns: LayoutColumn[]) => void;
  onClose: () => void;
}) {
  return (
    <Modal
      title="欄位設定"
      open={open}
      onCancel={onClose}
      footer={null}
      width={560}
      destroyOnHidden
    >
      <ColumnSettingsForm
        fields={fields}
        config={config}
        onCancel={onClose}
        onSubmit={(columns) => {
          onSubmit(columns);
          onClose();
        }}
      />
    </Modal>
  );
}
