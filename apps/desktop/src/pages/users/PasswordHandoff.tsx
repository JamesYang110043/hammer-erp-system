import { Result, Typography } from 'antd';

/** 建立帳號、重設密碼成功後顯示臨時密碼，提醒管理者交給使用者。 */
export function PasswordHandoff({
  title,
  username,
  password,
}: {
  title: string;
  username: string;
  password: string;
}) {
  return (
    <Result
      status="success"
      title={title}
      subTitle="請將臨時密碼交給使用者。使用者第一次登入時必須修改密碼；關閉視窗後就無法再查看此密碼。"
      extra={
        <div style={{ textAlign: 'left', display: 'inline-block' }}>
          <div>
            帳號：<Typography.Text strong>{username}</Typography.Text>
          </div>
          <div>
            臨時密碼：
            <Typography.Text
              strong
              code
              copyable={{ text: password, tooltips: ['複製', '已複製'] }}
            >
              {password}
            </Typography.Text>
          </div>
        </div>
      }
    />
  );
}
