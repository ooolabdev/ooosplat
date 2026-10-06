import { useEffect, useState } from 'react';
import { getMcpSettings, selectMcpInputRoot, setMcpSettings } from '../lib/backend';
import type { McpConnection, McpSettings } from '../types/tasks';
import { useI18n } from '../i18n';

export function McpSettingsPanel() {
  const { locale } = useI18n();
  const zh = locale === 'zh-CN';
  const [connection, setConnection] = useState<McpConnection | null>(null);
  const [settings, setSettings] = useState<McpSettings>({ enabled: false, port: 39877, inputRoots: [] });
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  useEffect(() => {
    let disposed = false;
    void getMcpSettings().then(value => { if (!disposed) { setConnection(value); setSettings(value.settings); } }).catch(() => { if (!disposed) setMessage(zh ? '无法读取 MCP 设置' : 'Could not read MCP settings'); });
    return () => { disposed = true; };
  }, [zh]);
  const apply = async (next: McpSettings) => {
    setBusy(true); setMessage(null);
    try { const value = await setMcpSettings(next); setConnection(value); setSettings(value.settings); }
    catch (error) { setMessage(error instanceof Error ? error.message : JSON.stringify(error)); }
    finally { setBusy(false); }
  };
  const copy = async () => {
    if (!connection?.token || !connection.address) return;
    try {
      await navigator.clipboard.writeText(JSON.stringify({ mcpServers: { ooosplat: { url: connection.address, headers: { Authorization: `Bearer ${connection.token}` } } } }, null, 2));
      setMessage(zh ? '连接配置已复制，请妥善保管 token。' : 'Configuration copied. Keep the token private.');
    } catch { setMessage(zh ? '无法复制，请手动复制下方配置。' : 'Could not copy. Select the configuration below.'); }
  };
  return <section>
    <div className="settings-section-heading"><h3>{zh ? '本地 AI Agent 连接' : 'Local AI Agent connection'}</h3><p>{zh ? '允许本机 MCP 客户端创建、启动和查看任务。云端 Agent 无法直接访问此电脑的 localhost。' : 'Allow local MCP clients to create, start and inspect tasks. Cloud agents cannot directly access this computer’s localhost.'}</p></div>
    <div className="settings-row"><div><strong>{zh ? '启用 MCP' : 'Enable MCP'}</strong><p>{zh ? '关闭连接入口不会取消已接受的生成任务。' : 'Disabling the connection keeps accepted generation tasks running.'}</p></div>
      <button type="button" role="switch" aria-label={zh ? '启用 MCP' : 'Enable MCP'} aria-checked={connection?.listening ?? false} disabled={busy} className={connection?.listening ? 'settings-switch enabled' : 'settings-switch'} onClick={() => void apply({ ...settings, enabled: !connection?.listening })}><span /></button>
    </div>
    <label className="field-label" htmlFor="mcp-port">{zh ? '本地端口' : 'Local port'}</label><input id="mcp-port" type="number" min={1} max={65535} value={settings.port} disabled={busy} onChange={event => setSettings(current => ({ ...current, port: Number(event.target.value) }))} />
    <div className="settings-row"><strong>{zh ? '授权素材目录' : 'Authorized input directories'}</strong><button type="button" disabled={busy} onClick={() => { void selectMcpInputRoot().then(root => { if (root) setSettings(current => ({ ...current, inputRoots: [...new Set([...current.inputRoots, root])] })); }).catch(() => setMessage(zh ? '无法选择目录' : 'Could not select a directory')); }}>{zh ? '添加目录' : 'Add directory'}</button></div>
    <p>{zh ? '未授权目录时，Agent 不能创建或启动任务。' : 'Agents cannot create or start tasks until an input directory is authorized.'}</p>
    {settings.inputRoots.map(root => <div className="settings-row" key={root}><span className="project-path">{root}</span><button type="button" disabled={busy} onClick={() => setSettings(current => ({ ...current, inputRoots: current.inputRoots.filter(item => item !== root) }))}>{zh ? '移除' : 'Remove'}</button></div>)}
    <button type="button" disabled={busy || !Number.isInteger(settings.port) || settings.port < 1 || settings.port > 65535} onClick={() => void apply(settings)}>{zh ? '保存设置' : 'Save settings'}</button>
    <p role="status">{connection?.error ?? message ?? (connection?.listening ? (zh ? '正在监听本机连接' : 'Listening locally') : (zh ? 'MCP 已关闭' : 'MCP disabled'))}</p>
    {connection?.listening && <><p className="project-path">{connection.address}</p><button type="button" onClick={() => void copy()}>{zh ? '复制客户端配置' : 'Copy client configuration'}</button><details><summary>{zh ? '查看连接配置' : 'View connection configuration'}</summary><pre className="mcp-configuration">{JSON.stringify({ mcpServers: { ooosplat: { url: connection.address, headers: { Authorization: `Bearer ${connection.token}` } } } }, null, 2)}</pre></details><p>{zh ? '应用重启或重新启用后 token 会更新，请重新复制配置。客户端的配置格式可能有所不同。' : 'The token changes after restarting or re-enabling the app. Copy the updated configuration. Client configuration formats may differ.'}</p></>}
  </section>;
}
