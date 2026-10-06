// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LanguageProvider } from '../i18n';
const mocks = vi.hoisted(() => ({ get: vi.fn(), set: vi.fn(), copy: vi.fn(), select: vi.fn() }));
vi.mock('../lib/backend', () => ({ getMcpSettings: mocks.get, setMcpSettings: mocks.set, selectMcpInputRoot: mocks.select }));
import { McpSettingsPanel } from './McpSettingsPanel';
const settings = { enabled: true, port: 39911, inputRoots: ['\\\\?\\E:\\Media'] };
const defaultInputRoot = '\\\\?\\E:\\Projects\\Inputs';
const visibleDefaultInputRoot = 'E:\\Projects\\Inputs';
let root: Root, container: HTMLDivElement;
const button = (text: string) => [...container.querySelectorAll<HTMLButtonElement>('button')].find(button => button.textContent === text)!;
beforeEach(async () => {
  (globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
  window.localStorage.setItem('ooo-splat-language', 'zh-CN');
  (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ = {};
  vi.resetAllMocks();
  mocks.get.mockResolvedValue({ settings, listening: true, address: 'http://127.0.0.1:39911/mcp', defaultInputRoot, error: null });
  mocks.set.mockImplementation(async next => ({ settings: next, listening: next.enabled, address: next.enabled ? 'http://127.0.0.1:39911/mcp' : null, defaultInputRoot, error: null }));
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: mocks.copy } });
  container = document.createElement('div'); document.body.append(container); root = createRoot(container);
  await act(async () => root.render(<LanguageProvider><McpSettingsPanel /></LanguageProvider>));
});
afterEach(async () => {
  await act(async () => root.unmount()); container.remove(); window.localStorage.clear();
  delete (window as typeof window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
});
describe('token-free MCP settings', () => {
  it('copies a URL-only client configuration and a directly usable Codex TOML block', async () => {
    await act(async () => button('复制客户端配置').click());
    expect(JSON.parse(mocks.copy.mock.calls.at(-1)![0])).toEqual({ mcpServers: { ooosplat: { url: 'http://127.0.0.1:39911/mcp' } } });
    await act(async () => button('复制 Codex 配置').click());
    expect(mocks.copy).toHaveBeenLastCalledWith('[mcp_servers.ooosplat]\nurl = "http://127.0.0.1:39911/mcp"\n');
    expect(container.textContent).not.toContain('Bearer');
    expect(container.textContent).not.toContain('token');
  });
  it('keeps the same URL when disabling and re-enabling the service', async () => {
    const toggle = () => container.querySelector<HTMLButtonElement>('[role="switch"]')!;
    await act(async () => toggle().click());
    expect(toggle().getAttribute('aria-checked')).toBe('false');
    await act(async () => toggle().click());
    expect(toggle().getAttribute('aria-checked')).toBe('true');
    await act(async () => button('复制 Codex 配置').click());
    expect(mocks.copy).toHaveBeenLastCalledWith('[mcp_servers.ooosplat]\nurl = "http://127.0.0.1:39911/mcp"\n');
  });

  it('shows the automatic default directory separately and does not persist it as an extra authorization', async () => {
    expect(container.querySelector('.mcp-default-root')?.textContent).toContain(visibleDefaultInputRoot);
    expect(container.querySelector('.mcp-settings')?.textContent).not.toContain('\\\\?\\');
    expect(container.querySelector('.mcp-default-root')?.textContent).toContain('自动授权');
    expect([...container.querySelectorAll('.mcp-default-root button')].some(item => item.textContent === '移除')).toBe(false);
    await act(async () => button('复制目录路径').click());
    expect(mocks.copy).toHaveBeenLastCalledWith(visibleDefaultInputRoot);
    await act(async () => button('保存设置').click());
    expect(mocks.set).toHaveBeenLastCalledWith(settings);
  });

  it('keeps user-added folders editable without changing the default folder', async () => {
    mocks.select.mockResolvedValue('E:\\More\\Images');
    await act(async () => button('添加目录').click());
    expect(container.querySelectorAll('.mcp-root-list li')).toHaveLength(2);
    await act(async () => button('保存设置').click());
    expect(mocks.set).toHaveBeenLastCalledWith({ ...settings, inputRoots: ['\\\\?\\E:\\Media', 'E:\\More\\Images'] });
    expect(container.querySelector('.mcp-default-root')?.textContent).toContain(visibleDefaultInputRoot);
  });
});
