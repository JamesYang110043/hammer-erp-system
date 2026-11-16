import { App, Button, List, Popconfirm, Space, Typography } from 'antd';

import { mutationErrorMessage } from '../../api/errors';
import { useDeleteEntity, useRecalculateEntity, useUpdateEntity } from '../../api/schema';
import type { EntityDefinition } from '../../bindings';
import { toUpdateEntityRequest } from '../../schema/entities';
import { referencingFields } from '../../schema/errorDetails';
import type { ReferencingField } from '../../schema/errorDetails';

/** 刪除被拒絕時的說明：列出引用它的關聯欄位。 */
export function ReferencingFieldsList({ fields }: { fields: ReferencingField[] }) {
  return (
    <FieldRefList
      intro={`以下 ${fields.length} 個關聯欄位還指向這個資料類別，請先停用或修改這些欄位：`}
      fields={fields}
    />
  );
}

/** 列出欄位（資料類別名稱、代碼／欄位名稱、代碼），前面是說明 `intro`。 */
export function FieldRefList({ intro, fields }: { intro: string; fields: ReferencingField[] }) {
  return (
    <>
      <Typography.Paragraph>{intro}</Typography.Paragraph>
      <List
        size="small"
        bordered
        dataSource={fields}
        rowKey={(f) => `${f.entity_key}.${f.field_key}`}
        renderItem={(f) => (
          <List.Item>
            <Space size={4} wrap>
              <Typography.Text strong>{f.entity_label}</Typography.Text>
              <Typography.Text code>{f.entity_key}</Typography.Text>
              <span>／</span>
              <Typography.Text>{f.field_label}</Typography.Text>
              <Typography.Text code>{f.field_key}</Typography.Text>
            </Space>
          </List.Item>
        )}
      />
    </>
  );
}

/**
 * 資料類別的操作：編輯、欄位設定、停用／啟用、刪除（需要 `schema.manage`，伺服器檢查）。
 *
 * 系統資料類別不可停用、不可刪除，不顯示這兩個按鈕。
 * 「重新計算」只有啟用中的自訂主檔、單據才有（明細隨主表一起重算，docs/architecture.md §7.6）。
 */
export function EntityActions({
  entity,
  onEdit,
  onOpenFields,
}: {
  entity: EntityDefinition;
  onEdit: () => void;
  onOpenFields: () => void;
}) {
  const { message, modal } = App.useApp();
  const updateEntity = useUpdateEntity();
  const deleteEntity = useDeleteEntity();
  const recalculate = useRecalculateEntity();
  const canRecalculate = !entity.is_system && entity.is_active && entity.kind !== 'detail';

  const runRecalculate = () =>
    recalculate.mutateAsync(entity.key).then(
      (result) =>
        void modal.success({
          title: `「${entity.label}」重新計算完成`,
          content: (
            <>
              <div>
                處理 {result.processed} 筆，其中 {result.changed} 筆有變動。
              </div>
              {result.with_errors > 0 && (
                <div>有 {result.with_errors} 筆資料的公式計算錯誤，請開啟資料查看錯誤原因。</div>
              )}
            </>
          ),
          okText: '知道了',
        }),
      (err: unknown) => void message.error(mutationErrorMessage(err)),
    );

  const setActive = (isActive: boolean) =>
    updateEntity
      .mutateAsync({
        key: entity.key,
        req: toUpdateEntityRequest(entity, { is_active: isActive }),
      })
      .then(
        () =>
          void message.success(
            isActive ? `已啟用「${entity.label}」` : `已停用「${entity.label}」`,
          ),
        (err: unknown) => void message.error(mutationErrorMessage(err)),
      );

  const remove = () =>
    deleteEntity.mutateAsync(entity).then(
      () => void message.success(`已刪除「${entity.label}」`),
      (err: unknown) => {
        const fields = referencingFields(err);
        if (fields) {
          modal.error({
            title: `無法刪除「${entity.label}」`,
            content: <ReferencingFieldsList fields={fields} />,
            okText: '知道了',
            width: 520,
          });
        } else {
          void message.error(mutationErrorMessage(err));
        }
      },
    );

  return (
    <Space size={0} wrap>
      <Button type="link" size="small" onClick={onEdit}>
        編輯
      </Button>
      <Button type="link" size="small" onClick={onOpenFields}>
        欄位設定
      </Button>
      {canRecalculate && (
        <Popconfirm
          title={`確定要重新計算「${entity.label}」？`}
          description="依目前的公式重新計算所有資料（含明細）的公式欄位，資料多時需要一些時間。"
          okText="重新計算"
          cancelText="取消"
          onConfirm={runRecalculate}
        >
          <Button type="link" size="small" loading={recalculate.isPending}>
            重新計算
          </Button>
        </Popconfirm>
      )}
      {!entity.is_system &&
        (entity.is_active ? (
          <Popconfirm
            title={`確定要停用「${entity.label}」？`}
            description="停用後不會出現在選單，也無法開啟資料；資料會保留，之後可以重新啟用。"
            okText="停用"
            okButtonProps={{ danger: true }}
            cancelText="取消"
            onConfirm={() => setActive(false)}
          >
            <Button type="link" size="small" danger loading={updateEntity.isPending}>
              停用
            </Button>
          </Popconfirm>
        ) : (
          <Button
            type="link"
            size="small"
            loading={updateEntity.isPending}
            onClick={() => void setActive(true)}
          >
            啟用
          </Button>
        ))}
      {!entity.is_system && (
        <Popconfirm
          title={`確定要刪除「${entity.label}」？`}
          description={
            <>
              {entity.kind !== 'detail' && <div>它的明細子表會一起刪除。</div>}
              <div>刪除後無法復原，代碼也不能再使用。</div>
            </>
          }
          okText="刪除"
          okButtonProps={{ danger: true }}
          cancelText="取消"
          onConfirm={remove}
        >
          <Button type="link" size="small" danger loading={deleteEntity.isPending}>
            刪除
          </Button>
        </Popconfirm>
      )}
    </Space>
  );
}
