// Vite alias active only for the test frontend. All application IPC, including
// classification/inspection/create/start/continue, still reaches real Rust.
function take() {
  const queue = (window as any).__e2eDialogResults;
  if (!queue?.length) throw new Error('Unexpected native dialog: declare its result in the test');
  return queue.shift();
}
export async function open(_options?: unknown): Promise<string[] | string | null> { return take(); }
export async function save(_options?: unknown): Promise<string | null> { return take(); }
export async function confirm(_message: string, _options?: unknown): Promise<boolean> { return take(); }
