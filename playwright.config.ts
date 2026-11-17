import { defineConfig } from '@playwright/test';

const reportRun = process.env.OOOSPLAT_E2E_REPORT_RUN;
if (reportRun && !/^[a-zA-Z0-9-]+$/.test(reportRun)) throw new Error('Invalid E2E report identity');
const output = reportRun ? `.cache/e2e/artifacts/${reportRun}` : '.cache/e2e';

export default defineConfig({
  testDir: './tests/e2e/specs',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  timeout: 120_000,
  expect: { timeout: 30_000 },
  outputDir: `${output}/results`,
  reporter: [['list'], ['html', { outputFolder: `${output}/report`, open: 'never' }]],
  use: { trace: 'retain-on-failure', screenshot: 'only-on-failure' },
});
