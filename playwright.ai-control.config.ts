import { defineConfig, devices } from "@playwright/test";
const baseURL = "http://127.0.0.1:" + (process.env.HOMEBASE_UI_PORT || "1429");
export default defineConfig({
  testDir: "./evals/ai-control-ui",
  testMatch: "*.spec.ts",
  outputDir: process.env.HOMEBASE_UI_RESULTS || ".tmp/ai-control-ui-results",
  timeout: 30000,
  workers: 1,
  reporter: "list",
  use: {
    ...devices["Desktop Edge"],
    channel: "msedge",
    // Isolated profiles may not share the machine's browser lookup environment.
    launchOptions: process.env.HOMEBASE_UI_BROWSER_EXE
      ? { executablePath: process.env.HOMEBASE_UI_BROWSER_EXE }
      : {},
    timezoneId: "Asia/Shanghai",
    baseURL,
    screenshot: "only-on-failure",
    trace: "retain-on-failure",
  },
  webServer: {
    command: "node evals/ai-control-ui/server.mjs",
    url: baseURL + "/evals/ai-control-ui/index.html",
    reuseExistingServer: process.env.HOMEBASE_UI_REUSE_SERVER === "1",
    timeout: 120000,
  },
});
