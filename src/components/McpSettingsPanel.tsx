import { useEffect, useState } from 'react';
import { getMcpSettings, selectMcpInputRoot, setMcpSettings } from '../lib/backend';
import { pipelineErrorMessage } from '../lib/pipelineError';
import { displayPath } from '../lib/displayPath';
import type { McpConnection, McpSettings } from '../types/tasks';
import { useI18n } from '../i18n';

export function McpSettingsPanel() {
  const { locale } = useI18n();
  const desktop = '__TAURI_INTERNALS__' in window;
  const zh = locale === 'zh-CN';
  const [connection, setConnection] = useState<McpConnection | null>(null);
  const [settings, setSettings] = useState<McpSettings>({ enabled: false, port: 39877, inputRoots: [] });
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    void getMcpSettings().then(value => { if (!disposed) { setConnection(value); setSettings(value.settings); } })
      .catch(() => { if (!disposed) setMessage(zh ? '无法读取 MCP 设置' : 'Could not read MCP settings'); });
    return () => { disposed = true; };
  }, [zh]);
  const apply = async (next: McpSettings) => {
    setBusy(true); setMessage(null);
    try { const value = await setMcpSettings(next); setConnection(value); setSettings(value.settings); }
    catch (error) { setMessage(pipelineErrorMessage(error) ?? (zh ? '无法保存 MCP 设置' : 'Could not save MCP settings')); }
    finally { setBusy(false); }
  };
  const addRoot = async () => {
    try {
      const root = await selectMcpInputRoot();
      if (root) setSettings(current => ({ ...current, inputRoots: [...new Set([...current.inputRoots, root])] }));
    } catch { setMessage(zh ? '无法选择目录' : 'Could not select a directory'); }
  };
  const clientConfiguration = connection?.address ? JSON.stringify({ mcpServers: { ooosplat: { url: connection.address } } }, null, 2) : '';
  const codexConfiguration = connection?.address ? `[mcp_servers.ooosplat]\nurl = ${JSON.stringify(connection.address)}\n` : '';
  const copy = async (text: string) => {
    if (!text) return;
    try { await navigator.clipboard.writeText(text); setMessage(zh ? '已复制。' : 'Copied.'); }
    catch { setMessage(zh ? '无法复制，请手动选择并复制文本。' : 'Could not copy. Select and copy the text manually.'); }
  };
  const defaultRoot = connection?.defaultInputRoot;
  const visibleDefaultRoot = defaultRoot ? displayPath(defaultRoot) : null;
  const canSave = desktop && !busy && Number.isInteger(settings.port) && settings.port >= 1 && settings.port <= 65535;
  return <section className="mcp-settings">
    <div className="settings-section-heading"><h3>{zh ? '本地 AI Agent 连接' : 'Local AI Agent connection'}</h3><p>{zh ? '让本机 MCP 客户端操作 OOOSplat 中的生成任务。' : 'Let local MCP clients work with generation tasks in OOOSplat.'}</p></div>

    <section className="mcp-section" aria-label={zh ? 'MCP 服务' : 'MCP service'}>
      <div className="mcp-control-row"><div><strong>{zh ? '启用 MCP' : 'Enable MCP'}</strong><p>{zh ? '关闭连接入口不会取消已接受的生成任务。' : 'Disabling the connection keeps accepted generation tasks running.'}</p></div>
        <button type="button" role="switch" aria-label={zh ? '启用 MCP' : 'Enable MCP'} aria-checked={connection?.listening ?? false} disabled={busy || !desktop} className={connection?.listening ? 'settings-switch enabled' : 'settings-switch'} onClick={() => void apply({ ...settings, enabled: !connection?.listening })}><span /></button>
      </div>
      <div className="mcp-control-row"><div><label htmlFor="mcp-port">{zh ? '本地端口' : 'Local port'}</label><p>{zh ? '修改后点击保存设置。' : 'Save settings to apply a new port.'}</p></div><input className="mcp-port" id="mcp-port" type="number" min={1} max={65535} value={settings.port} disabled={busy || !desktop} onChange={event => setSettings(current => ({ ...current, port: Number(event.target.value) }))} /></div>
    </section>

    <section className="mcp-section" aria-label={zh ? '素材授权目录' : 'Authorized input directories'}>
      <div className="mcp-section-heading"><h4>{zh ? '素材授权目录' : 'Authorized input directories'}</h4><button className="mcp-action" type="button" disabled={busy || !desktop} onClick={() => void addRoot()}>{zh ? '添加目录' : 'Add directory'}</button></div>
      <p className="mcp-note">{zh ? '将视频或图片放入默认目录即可使用，也可以添加其他素材目录。' : 'Place videos or images in the default directory, or authorize additional folders.'}</p>
      {visibleDefaultRoot ? <div className="mcp-default-root"><div className="mcp-root-heading"><strong>{zh ? '默认素材目录' : 'Default input directory'}</strong><span>{zh ? '自动授权' : 'Automatically authorized'}</span></div><p className="mcp-root-path" title={visibleDefaultRoot}>{visibleDefaultRoot}</p><p className="mcp-note">{zh ? '启用 MCP 时自动创建，位置为项目根目录下的 Inputs。' : 'Created when MCP is enabled, under the projects root as Inputs.'}</p><button className="mcp-action" type="button" onClick={() => void copy(visibleDefaultRoot)}>{zh ? '复制目录路径' : 'Copy directory path'}</button></div> : <p className="mcp-note">{zh ? '默认目录会随桌面应用的项目根目录设置显示。' : 'The desktop app supplies the default directory from its projects root.'}</p>}
      {settings.inputRoots.length > 0 && <ul className="mcp-root-list" aria-label={zh ? '其他授权目录' : 'Additional authorized directories'}>{settings.inputRoots.map(root => { const visibleRoot = displayPath(root); return <li className="mcp-root-row" key={root}><p className="mcp-root-path" title={visibleRoot}>{visibleRoot}</p><button className="mcp-action" type="button" disabled={busy || !desktop} onClick={() => setSettings(current => ({ ...current, inputRoots: current.inputRoots.filter(item => item !== root) }))}>{zh ? '移除' : 'Remove'}</button></li>; })}</ul>}
      <div className="mcp-save-row"><button className="mcp-action" type="button" disabled={!canSave} onClick={() => void apply(settings)}>{zh ? '保存设置' : 'Save settings'}</button><p className="mcp-note">{zh ? '运行中的任务继续使用提交时的配置。' : 'Running tasks keep their submitted configuration.'}</p></div>
    </section>

    <section className="mcp-section mcp-client-section" aria-label={zh ? '客户端连接' : 'Client connection'}>
      <div className="mcp-section-heading"><h4>{zh ? '客户端连接' : 'Client connection'}</h4><span className={`mcp-connection-state${connection?.listening ? ' listening' : ''}`}>{connection?.listening ? (zh ? '已启用' : 'Enabled') : (zh ? '已关闭' : 'Disabled')}</span></div>
      {connection?.listening && connection.address ? <>
        <p className="mcp-note">{zh ? '连接地址' : 'Connection address'}</p><code className="mcp-endpoint">{connection.address}</code>
        <div className="mcp-copy-actions"><button className="mcp-action" type="button" disabled={busy} onClick={() => void copy(clientConfiguration)}>{zh ? '复制客户端配置' : 'Copy client configuration'}</button><button className="mcp-action" type="button" disabled={busy} onClick={() => void copy(codexConfiguration)}>{zh ? '复制 Codex 配置' : 'Copy Codex configuration'}</button></div>
        <details className="mcp-config-details"><summary>{zh ? '查看连接配置' : 'View connection configuration'}</summary><h5>Codex</h5><pre className="mcp-configuration">{codexConfiguration}</pre><h5>{zh ? '通用 MCP 客户端' : 'Generic MCP client'}</h5><pre className="mcp-configuration">{clientConfiguration}</pre></details>
        <p className="mcp-note">{zh ? '本机连接无需密钥。地址和端口不变时，重启应用后无需更新配置。' : 'Local connections need no credentials. Restarting does not require configuration changes unless the address or port changes.'}</p>
      </> : <p className="mcp-note">{zh ? '启用 MCP 后可以复制连接配置。' : 'Enable MCP to copy the client configuration.'}</p>}
      <p className="mcp-note">{zh ? '云端 Agent 无法直接访问此电脑的 localhost。' : 'Cloud agents cannot directly access this computer’s localhost.'}</p>
    </section>
    {!desktop && <p className="mcp-note">{zh ? '请在 OOOSplat 桌面应用中启用本地连接。' : 'Enable local connections in the OOOSplat desktop app.'}</p>}
    {(connection?.error || message) && <p className={`mcp-feedback${connection?.error ? ' error' : ''}`} role="status">{connection?.error ?? message}</p>}
  </section>;
}
