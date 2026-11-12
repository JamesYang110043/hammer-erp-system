import { Card, Empty } from 'antd';

/** 尚未開發的模組。 */
export function PlaceholderPage({ title }: { title: string }) {
  return (
    <Card title={title}>
      <Empty description="此功能尚未開放" />
    </Card>
  );
}
