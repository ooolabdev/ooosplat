// Minimal real Streamable HTTP client. No mocked MCP responses or application
// service shortcuts. Both JSON and SSE responses are accepted by the protocol.
export class McpClient {
  private session: string | null = null;
  private id = 0;
  constructor(private endpoint: string) {}
  private async request(method: string, params: unknown, notification = false) {
    const response = await fetch(this.endpoint, {
      method: 'POST', headers: {
        'Content-Type': 'application/json', Accept: 'application/json, text/event-stream',
        'MCP-Protocol-Version': '2025-03-26', ...(this.session ? { 'Mcp-Session-Id': this.session } : {}),
      }, body: JSON.stringify({ jsonrpc: '2.0', ...(notification ? {} : { id: ++this.id }), method, params }),
      signal: AbortSignal.timeout(30_000),
    });
    const session = response.headers.get('mcp-session-id'); if (session) this.session = session;
    if (!response.ok) throw new Error(`MCP HTTP ${response.status}: ${await response.text()}`);
    if (notification) return;
    const text = await response.text();
    const value = response.headers.get('content-type')?.includes('text/event-stream')
      ? text.split('\n').filter(line => line.startsWith('data:')).map(line => JSON.parse(line.slice(5))).find(value => value.id === this.id)
      : JSON.parse(text);
    if (!value || value.error) throw new Error(`MCP protocol error: ${JSON.stringify(value)}`);
    return value.result;
  }
  async initialize() {
    await this.request('initialize', { protocolVersion: '2025-03-26', capabilities: {}, clientInfo: { name: 'ooosplat-e2e', version: '1' } });
    await this.request('notifications/initialized', {}, true);
  }
  async call(name: string, args: unknown) {
    const result = await this.request('tools/call', { name, arguments: args });
    if (result.isError) throw new Error(`MCP tool rejected request: ${JSON.stringify(result)}`);
    return result.structuredContent ?? JSON.parse(result.content.find((item: { type: string }) => item.type === 'text').text);
  }
}
