import type { TranslationKey } from "../i18n";
import type { FailureContext } from "./pipelineError";
import { classifiedSummary } from "./failureGuidance";

// Keep operational details in the tooltip, not in the task-card layout.
export function errorSummaryKey(message: string, context?: FailureContext): TranslationKey | null {
  // Admission explanations mention COLMAP but are not engine failures.
  if (message === '补拍暂不支持导入 COLMAP 重建结果，请添加视频或图片。') return 'reshoot.colmapUnsupported';
  if (message === '旧任务的历史参数或初始化数据不完整，无法安全继续。原项目和已有结果保持不变。') return 'project.legacyCannotContinue';
  if (context?.classificationVersion != null) return classifiedSummary(context);
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
