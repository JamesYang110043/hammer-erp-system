import { Alert, Form, Modal, Select, Table, Tag, Typography } from 'antd';
import { useState } from 'react';

import { useChangeFieldType } from '../../api/schema';
import type { DataType, FieldDefinition } from '../../bindings';
import { conversionFailures, formatValue } from '../../schema/errorDetails';
import type { ConversionFailure } from '../../schema/errorDetails';
import { toChangeTypeRequest, toFormValues } from '../../schema/fieldForm';
import type { FieldFormValues } from '../../schema/fieldForm';
import { DATA_TYPE_LABELS, SUPPORTED_DATA_TYPES } from '../../schema/fieldSettings';
import { applyFormError } from '../users/formErrors';
import { TypeSettingsFields } from './FieldInputs';

type FormValues = Omit<Partial<FieldFormValues>, 'data_type'> & { data_type?: DataType };

/**
 * 變更自訂欄位的資料型態（docs/architecture.md §7.4）。
 *
 * 伺服器會轉換所有既有資料（含已刪除）；只要有一筆無法轉換就整批拒絕，
 * 此時列出無法轉換的資料（{@link ConversionFailures}）。
 */
export function ChangeTypeModal({
  entityKey,
  field,
  onClose,
}: {
  entityKey: string;
  field: FieldDefinition | undefined;
  onClose: () => void;
}) {
  const [form] = Form.useForm<FormValues>();
  const changeType = useChangeFieldType(entityKey);
  const [error, setError] = useState<string>();
  const [failures, setFailures] = useState<{ total: number; records: ConversionFailure[] }>();
  const dataType = Form.useWatch('data_type', form);

  const close = () => {
    setError(undefined);
    setFailures(undefined);
    changeType.reset();
    onClose();
  };

  const submit = (values: FormValues) => {
    if (!field || !values.data_type) {
      return;
    }
    setError(undefined);
    setFailures(undefined);
    changeType.mutate(
      { fieldKey: field.field_key, req: toChangeTypeRequest(field, values.data_type, values) },
      {
        onSuccess: close,
        onError: (err) => {
          const failed = conversionFailures(err);
          setFailures(failed);
          setError(failed ? undefined : applyFormError(form, err));
        },
      },
    );
  };

  return (
    <Modal
      title={field ? `變更型態：${field.label}` : '變更型態'}
      open={field !== undefined}
      onCancel={close}
      onOk={() => form.submit()}
      okText="變更"
      okButtonProps={{ loading: changeType.isPending }}
      destroyOnHidden
      width={720}
    >
      {field && (
        <Form
          form={form}
          layout="vertical"
          onFinish={submit}
          disabled={changeType.isPending}
          initialValues={{ ...toFormValues(field), data_type: undefined }}
          onValuesChange={() => setFailures(undefined)}
          preserve={false}
        >
          <Alert
            showIcon
            type="warning"
            message="會轉換這個欄位的所有既有資料（含已刪除的資料）"
            description="轉成文字、文字互轉、整數轉小數、單選轉多選一定成功；其他轉換只要有一筆資料無法轉換，就不會變更。"
            style={{ marginBottom: 16 }}
          />
          <Form.Item label="目前型態">
            <Typography.Text>{DATA_TYPE_LABELS[field.data_type]}</Typography.Text>
          </Form.Item>
          <Form.Item
            label="新的型態"
            name="data_type"
            rules={[{ required: true, message: '請選擇新的型態' }]}
          >
            <Select
              options={SUPPORTED_DATA_TYPES.filter((t) => t !== field.data_type).map((t) => ({
                value: t,
                label: DATA_TYPE_LABELS[t],
              }))}
            />
          </Form.Item>
          {dataType && <TypeSettingsFields dataType={dataType} />}
          {failures && <ConversionFailures {...failures} />}
          {error && <Alert showIcon type="error" message={error} />}
        </Form>
      )}
    </Modal>
  );
}

/** 型態變更失敗時，無法轉換的資料清單。 */
export function ConversionFailures({
  total,
  records,
}: {
  total: number;
  records: ConversionFailure[];
}) {
  const shown = records.length < total ? `，以下列出前 ${records.length} 筆` : '';
  return (
    <Alert
      showIcon
      type="error"
      message={`有 ${total} 筆資料無法轉換，型態未變更${shown}`}
      description={
        <Table<ConversionFailure>
          rowKey="id"
          size="small"
          pagination={false}
          scroll={{ y: 240 }}
          dataSource={records}
          columns={[
            {
              title: '資料 ID',
              dataIndex: 'id',
              render: (id: string, r) => (
                <>
                  <Typography.Text copyable style={{ fontSize: 12 }}>
                    {id}
                  </Typography.Text>
                  {r.deleted && <Tag style={{ marginInlineStart: 4 }}>已刪除</Tag>}
                </>
              ),
            },
            { title: '原本的值', dataIndex: 'value', render: (v: unknown) => formatValue(v) },
            { title: '原因', dataIndex: 'message' },
          ]}
        />
      }
    />
  );
}
