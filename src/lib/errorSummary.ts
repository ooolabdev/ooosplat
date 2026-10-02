import type { TranslationKey } from "../i18n";

// Keep operational details in the tooltip, not in the task-card layout.
export function errorSummaryKey(message: string): TranslationKey | null {
  if (/early eof|failed to load dataset|i\/?o error while loading dataset|图片读取不完整/i.test(message)) return "error.summaryDataset";
  if (/device.?lost|parent device is lost|vk_error_device_lost|dxgi_error_device_removed|显卡.*连接中断/i.test(message)) return "error.summaryDevice";
  if (/out of memory|\boom\b|显存不足|显存.*不够/i.test(message)) return /brush|gpu|vram|显存/i.test(message) ? "error.summaryGraphicsMemory" : "error.summaryMemory";
  if (/no space left|disk full|磁盘.*(不足|已满)/i.test(message)) return "error.summaryDisk";
  if (/access denied|permission denied|拒绝访问|权限不足/i.test(message)) return "error.summaryPermission";
  if (/timeout|timed out|超时/i.test(message)) return "error.summaryTimeout";
  if (/brush/i.test(message)) return "error.summaryTraining";
  if (/colmap|mapper/i.test(message)) return "error.summaryReconstruction";
  if (/ffprobe|ffmpeg/i.test(message)) return "error.summaryMedia";
  if (/no such file|file not found|文件.*(不存在|找不到)/i.test(message)) return "error.summaryFile";
  return null;
}

export function shortErrorMessage(message: string, limit: number): string {
  const firstLine = message.trim().split(/\r?\n/, 1)[0].trim();
  const characters = Array.from(firstLine);
  return characters.length > limit ? `${characters.slice(0, limit - 1).join("")}…` : firstLine;
}
