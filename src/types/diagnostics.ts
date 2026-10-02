export interface DiagnosticReport {
  schemaVersion: number;
  reportId: string;
  timestamp: string;
  appVersion: string;
  stage: string | null;
  engine: string | null;
  errorCode: string;
  reason: string;
  detail: string;
  logTail: string;
  logsTruncated: boolean;
  environment: {
    system: { name: string; version: string | null; arch: string };
    gpus: Array<{ name: string; driverVersion: string | null; totalMemoryMb: number | null; computeCapability: string | null }>;
    actualDevice: string | null;
  };
}
export interface ErrorReportDraft { draftId: string; report: DiagnosticReport }
export interface ErrorReportReceipt { reportId: string; accepted: boolean }
