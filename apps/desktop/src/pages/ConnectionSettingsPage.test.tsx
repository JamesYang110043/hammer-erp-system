import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';

import { commandError, mockCommands } from '../test/ipc';
import { renderWithProviders } from '../test/render';
import { ConnectionSettingsPage } from './ConnectionSettingsPage';

const SAVED_URL = 'http://localhost:8080';

describe('ConnectionSettingsPage', () => {
  it('shows_saved_server_url', async () => {
    mockCommands({ get_server_url: () => SAVED_URL });

    renderWithProviders(<ConnectionSettingsPage />);

    expect(await screen.findByLabelText('伺服器網址')).toHaveValue(SAVED_URL);
  });

  it('test_connection_success_shows_server_version', async () => {
    const invoke = mockCommands({
      get_server_url: () => SAVED_URL,
      health_check: () => ({ status: 'ok', version: '1.2.3', database: 'ok' }),
    });
    const user = userEvent.setup();
    renderWithProviders(<ConnectionSettingsPage />);

    const input = await screen.findByLabelText('伺服器網址');
    await user.clear(input);
    await user.type(input, 'http://192.168.1.10:8080');
    await user.click(screen.getByRole('button', { name: '測試連線' }));

    expect(await screen.findByText('連線成功，伺服器版本 v1.2.3')).toBeInTheDocument();
    // 測試連線使用輸入框的網址，不使用已儲存的網址
    expect(invoke).toHaveBeenCalledWith('health_check', { serverUrl: 'http://192.168.1.10:8080' });
    expect(invoke).not.toHaveBeenCalledWith('set_server_url', expect.anything());
  });

  it('test_connection_database_down_shows_warning', async () => {
    mockCommands({
      get_server_url: () => SAVED_URL,
      health_check: () => ({ status: 'error', version: '1.2.3', database: 'error' }),
    });
    const user = userEvent.setup();
    renderWithProviders(<ConnectionSettingsPage />);

    await user.click(await screen.findByRole('button', { name: '測試連線' }));

    expect(
      await screen.findByText('已連到伺服器（v1.2.3），但伺服器無法連線到資料庫'),
    ).toBeInTheDocument();
  });

  it('test_connection_network_error_shows_message', async () => {
    mockCommands({
      get_server_url: () => SAVED_URL,
      health_check: () => commandError({ kind: 'network', message: '無法連線到伺服器' }),
    });
    const user = userEvent.setup();
    renderWithProviders(<ConnectionSettingsPage />);

    await user.click(await screen.findByRole('button', { name: '測試連線' }));

    expect(await screen.findByText('無法連線到伺服器')).toBeInTheDocument();
  });
});
