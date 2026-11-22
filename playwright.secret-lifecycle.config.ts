import { defineConfig, devices } from "@playwright/test";
export default defineConfig({
  testDir: "./evals/secret-lifecycle-ui",
  testMatch: "*.spec.ts",
  outputDir: ".tmp/secret-lifecycle-ui-results",
  timeout: 60_000,
  workers: 1,
  reporter: "list",
  use: {
    ...devices["Desktop Edge"],
    channel: "msedge",
    timezoneId: "Asia/Shanghai",
    baseURL: "http://127.0.0.1:1428",
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  webServer: {
    command: "node evals/secret-lifecycle-ui/server.mjs",
    url: "http://127.0.0.1:1428/evals/secret-lifecycle-ui/index.html",
    reuseExistingServer: false,
    timeout: 300_000,
  },
});
