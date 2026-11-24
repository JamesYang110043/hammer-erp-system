import { TableOutlined, SplitCellsOutlined } from '@ant-design/icons';
import { Segmented } from 'antd';

import type { RecordView } from '../../records/masterSplit';

/** 頁首右上角的分割／表格檢視切換鈕（§8.7）。 */
export function RecordViewToggle({
  value,
  onChange,
}: {
  value: RecordView;
  onChange: (view: RecordView) => void;
}) {
  return (
    <Segmented<RecordView>
      aria-label="檢視方式"
      value={value}
      onChange={onChange}
      options={[
        { value: 'split', icon: <SplitCellsOutlined />, label: '分割' },
        { value: 'table', icon: <TableOutlined />, label: '表格' },
      ]}
    />
  );
}
