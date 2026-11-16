import { DeleteOutlined, UploadOutlined } from '@ant-design/icons';
import { App, Button, Progress, Space, Typography } from 'antd';
import { useState } from 'react';

import { formatFileSize, progressPercent, uploadAttachment } from '../../api/attachments';
import type { Attachment, UploadProgress } from '../../api/attachments';
import { errorMessage } from '../../api/client';
import { AttachmentLink } from './AttachmentLink';

/** 值有附件 id、但伺服器沒有回傳附件資訊（已刪除或資料不一致）時顯示的文字。 */
export const MISSING_ATTACHMENT_TEXT = '（附件不存在）';

/**
 * 附件欄位的輸入：上傳、顯示檔名與大小、下載、移除。值為附件 id。
 *
 * - 「上傳檔案」開啟選擇檔案的對話框，上傳中顯示進度；超過大小上限等錯誤以訊息顯示
 * - 上傳後附件尚未綁定，存檔時才寫入這筆資料；移除只清空欄位，存檔後舊附件才會被標記刪除
 * - 停用（唯讀者、唯讀欄位）時只能下載
 */
export function FileInput({
  value,
  onChange,
  initial,
  disabled,
  id,
  'aria-label': ariaLabel,
}: {
  value?: string;
  onChange?: (value: string | undefined) => void;
  /** 資料目前的附件（`Record.attachments`）。 */
  initial?: Attachment;
  disabled?: boolean;
  id?: string;
  'aria-label'?: string;
}) {
  const { message } = App.useApp();
  // 目前已知的附件：資料原本的附件，或剛上傳的附件
  const [uploaded, setUploaded] = useState<Attachment>();
  const [uploading, setUploading] = useState(false);
  const [progress, setProgress] = useState<UploadProgress>();

  const current = [uploaded, initial].find((a) => a !== undefined && a.id === value);

  const upload = async () => {
    setUploading(true);
    setProgress(undefined);
    try {
      const attachment = await uploadAttachment(setProgress);
      if (attachment) {
        setUploaded(attachment);
        onChange?.(attachment.id);
      }
    } catch (err) {
      message.error(`上傳失敗：${errorMessage(err)}`);
    } finally {
      setUploading(false);
      setProgress(undefined);
    }
  };

  return (
    <div id={id} role="group" aria-label={ariaLabel}>
      <Space direction="vertical" size={4} style={{ width: '100%', maxWidth: 480 }}>
        {value && (
          <Space size={8} wrap>
            {current ? (
              <>
                <AttachmentLink attachment={current} />
                <Typography.Text type="secondary">
                  {formatFileSize(current.size_bytes)}
                </Typography.Text>
              </>
            ) : (
              <Typography.Text type="secondary">{MISSING_ATTACHMENT_TEXT}</Typography.Text>
            )}
            <Button
              size="small"
              type="text"
              danger
              icon={<DeleteOutlined />}
              // 傳 false 會蓋過外層表單的停用（唯讀者），沒有要停用時傳 undefined
              disabled={disabled || uploading || undefined}
              onClick={() => onChange?.(undefined)}
            >
              移除
            </Button>
          </Space>
        )}
        {uploading && progress && (
          <div>
            <Typography.Text>{progress.file_name}</Typography.Text>
            <Progress
              percent={progressPercent(progress)}
              size="small"
              aria-label={`上傳進度 ${progress.file_name}`}
            />
          </div>
        )}
        <Button
          size="small"
          icon={<UploadOutlined />}
          loading={uploading}
          disabled={disabled}
          onClick={() => void upload()}
        >
          {uploading ? '上傳中' : value ? '更換檔案' : '上傳檔案'}
        </Button>
      </Space>
    </div>
  );
}
