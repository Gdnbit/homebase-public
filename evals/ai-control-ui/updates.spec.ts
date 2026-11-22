import { test, expect, type Page } from "@playwright/test";
import { navigate } from "./navigation";

const count = (page: Page, command: string) =>
  page.evaluate(
    (c) =>
      (window as any).__performance.calls.filter((s: string) => s === c).length,
    command,
  );
const go = (page: Page, name: string) =>
  navigate(page, name);
test.beforeEach(async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1"
      ? route.continue()
      : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await navigate(page, "概览");
  await expect(page.getByText("磁盘健康", { exact: true })).toBeVisible();
});

test("history manual refresh invalidates the cached session detail", async ({
  page,
}) => {
  await page.evaluate(() => {
    const { values } = (window as any).__performance;
    values.agents_history_all = {
      buckets: [],
      entries: [
        {
          agent_id: "codex",
          session_id: "synthetic",
          cwd: "synthetic",
          first_user_prompt: "Synthetic session",
          started_at: "2026-11-09T00:00:00Z",
          last_active_at: "2026-11-09T00:00:00Z",
          message_count: 1,
          file_path: "synthetic",
        },
      ],
    };
    values.agents_history_read = {
      agent_id: "codex",
      session_id: "synthetic",
      cwd: "synthetic",
      messages: [
        {
          role: "user",
          content: "Detail before refresh",
          timestamp: "2026-11-09T00:00:00Z",
        },
      ],
      stats: {
        total_input_tokens: 0,
        total_output_tokens: 0,
        message_count: 1,
      },
    };
  });
  await go(page, "历史对话");
  await page
    .getByRole("button")
    .filter({ hasText: "Synthetic session" })
    .click();
  await expect(
    page.getByText("Detail before refresh", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "关闭", exact: true }).click();
  await page.evaluate(() => {
    (
      window as any
    ).__performance.values.agents_history_read.messages[0].content =
      "Detail after refresh";
  });
  await page.getByRole("button", { name: "刷新", exact: true }).click();
  await expect.poll(() => count(page, "agents_history_all")).toBe(2);
  await page
    .getByRole("button")
    .filter({ hasText: "Synthetic session" })
    .click();
  await expect(
    page.getByText("Detail after refresh", { exact: true }),
  ).toBeVisible();
  expect(await count(page, "agents_history_read")).toBe(2);
});

test("knowledge refresh updates preview and tree once, including a previously cached inactive view", async ({
  page,
}) => {
  await page.evaluate(() => {
    const { values } = (window as any).__performance;
    values.wiki_list = [
      {
        rel_path: "wiki/notes/synthetic.md",
        absolute_path: "synthetic",
        title: "Synthetic note",
        kind: "note",
        size: 10,
        modified_ms: 1,
      },
    ];
    values.wiki_read = {
      rel_path: "wiki/notes/synthetic.md",
      absolute_path: "synthetic",
      exists: true,
      size: 10,
      content: "Preview before refresh",
      error: null,
    };
  });
  await go(page, "知识库");
  await page.getByRole("button").filter({ hasText: "Synthetic note" }).click();
  await expect(
    page.getByText("Preview before refresh", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    (window as any).__performance.values.wiki_read.content =
      "Preview after refresh";
  });
  await page.getByRole("button", { name: "刷新知识库", exact: true }).click();
  await expect(
    page.getByText("Preview after refresh", { exact: true }),
  ).toBeVisible();
  expect(await count(page, "wiki_read")).toBe(2);
  expect(await count(page, "wiki_list")).toBe(2);
  await page.getByRole("button", { name: /AI 知识库/ }).click();
  await expect.poll(() => count(page, "topology_load")).toBe(1);
  await page.getByRole("button", { name: /LLM Wiki/ }).click();
  expect(await count(page, "wiki_list")).toBe(2);
});

test("model completion ignores a late running poll, refreshes manifest once and ignores duplicate finish events", async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.model_download_reconcile = [
      {
        job_id: "synthetic-download",
        runtime_state: "managed",
        model_id: "faster-whisper-small",
        model_name: "Synthetic model",
        status: "running",
        phase: "downloading",
        progress_percent: 20,
      },
    ];
    f.values.model_download_get = () =>
      new Promise((resolve) => {
        f.releaseDownload = () =>
          resolve({
            job_id: "synthetic-download",
            status: "running",
            phase: "downloading",
            progress_percent: 20,
          });
      });
  });
  await go(page, "快捷面板");
  await expect.poll(() => count(page, "model_download_get")).toBe(1);
  const before = await count(page, "model_center_manifest");
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.model_center_manifest = { source_path: "synthetic", manifest_state: "loaded", schema_version: 1, updated_at: "synthetic", error: null, models: [{ id: "faster-whisper-small", state: "installed", local_path: "synthetic", path_exists: true, detail: null }] };
    f.emit("model-download", {
      kind: "finished",
      jobId: "synthetic-download",
      status: "succeeded",
      message: "Synthetic completed",
    });
    f.releaseDownload();
  });
  await expect
    .poll(() => count(page, "model_center_manifest"))
    .toBe(before + 1);
  await expect(page.getByText(/已验证 1/)).toBeVisible();
  await page.waitForTimeout(1800);
  expect(await count(page, "model_download_get")).toBe(1);
  await page.evaluate(() =>
    (window as any).__performance.emit("model-download", {
      kind: "finished",
      jobId: "synthetic-download",
      status: "succeeded",
    }),
  );
  await page.waitForTimeout(150);
  expect(await count(page, "model_center_manifest")).toBe(before + 1);
});

test("Studio completion waits for persistence and retains one stable listener while streaming", async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.studio_chat_send = {
      task_id: "synthetic-studio",
      session_id: "synthetic-session",
      cmd_preview: "synthetic",
    };
    f.values.studio_persist_message = (args: any) =>
      args.message.role === "user"
        ? null
        : new Promise((resolve) => {
            f.finishPersist = () => {
              f.values.studio_list_sessions = [
                {
                  session_id: "synthetic-session",
                  title: "Persisted synthetic session",
                  message_count: 2,
                  created_ms: 1,
                  updated_ms: 2,
                  chat_path: "synthetic",
                  article_path: null,
                },
              ];
              resolve(null);
            };
          });
  });
  await go(page, "文章生成");
  await page
    .getByRole("button", { name: /新建对话/ })
    .first()
    .click();
  const input = page.getByRole("textbox");
  await input.fill("Synthetic message only");
  await input.press("Enter");
  await expect.poll(() => count(page, "studio_chat_send")).toBe(1);
  const before = await count(page, "studio_list_sessions");
  const listenerCount = await page.evaluate(
    () =>
      (window as any).__performance.registrations.filter(
        (s: string) => s === "studio:event",
      ).length,
  );
  for (let i = 0; i < 3; i++) {
    await page.evaluate(
      (i) =>
        (window as any).__performance.emit("studio:event", {
          kind: "assistant_text",
          task_id: "synthetic-studio",
          text: `Synthetic chunk ${i}`,
        }),
      i,
    );
    await page.waitForTimeout(50);
  }
  expect(
    await page.evaluate(
      () =>
        (window as any).__performance.registrations.filter(
          (s: string) => s === "studio:event",
        ).length,
    ),
  ).toBe(listenerCount);
  await page.evaluate(() =>
    (window as any).__performance.emit("studio:event", {
      kind: "completed",
      task_id: "synthetic-studio",
      session_id: "synthetic-session",
      exit_code: 0,
    }),
  );
  await expect.poll(() => count(page, "studio_persist_message")).toBe(2);
  await page.waitForTimeout(150);
  expect(await count(page, "studio_list_sessions")).toBe(before);
  await page.evaluate(() => (window as any).__performance.finishPersist());
  await expect.poll(() => count(page, "studio_list_sessions")).toBe(before + 1);
  await go(page, "历史对话");
  await go(page, "文章生成");
  await expect(
    page.getByText("Persisted synthetic session", { exact: true }),
  ).toBeVisible();
});

test("manual refresh buttons update all governance queries and model catalog without duplicate calls", async ({
  page,
}) => {
  await go(page, "治理追溯");
  const commands = [
    "governance_audit_recent",
    "governance_snapshots_list",
    "governance_capability_matrix",
    "governance_config_ownership",
    "governance_interface_registry",
  ];
  for (const command of commands)
    await expect.poll(() => count(page, command)).toBe(1);
  await page.evaluate(() => {
    (window as any).__performance.values.governance_capability_matrix = {
      rows: [
        {
          actor_id: "Updated synthetic actor",
          actor_kind: "test",
          mcp_servers: [],
          skills: [],
          allowed_secrets: [],
          config_paths: [],
          risk: "synthetic",
        },
      ],
    };
  });
  await page.getByRole("button", { name: "刷新", exact: true }).click();
  for (const command of commands)
    await expect.poll(() => count(page, command)).toBe(2);
  await expect(
    page.getByText("Updated synthetic actor", { exact: true }),
  ).toBeVisible();
  await go(page, "快捷面板");
  await expect.poll(() => count(page, "openrouter_models_catalog")).toBe(1);
  await page.getByRole("button", { name: "刷新列表", exact: true }).click();
  await expect.poll(() => count(page, "openrouter_models_catalog")).toBe(2);
});

test("agent model save updates cached configuration across drawer tabs", async ({
  page,
}) => {
  await page.evaluate(async () => {
    const f = (window as any).__performance;
    f.values.agents_list = [
      {
        id: "codex",
        name: "Synthetic Codex",
        kind: "cli",
        config_path: "synthetic",
        config_exists: true,
        launch_subcommand: null,
        process_match: [],
      },
    ];
    f.values.agents_status = {
      id: "codex",
      state: "stopped",
      running: false,
      pid: null,
      config_exists: true,
      config_path: "synthetic",
      mcp_servers: [],
      last_seen: null,
      error: null,
    };
    f.values.agents_get_model = {
      agent_id: "codex",
      current: "synthetic-a",
      presets: [
        { value: "synthetic-a", label: "Synthetic A", hint: "test" },
        { value: "synthetic-b", label: "Synthetic B", hint: "test" },
      ],
      supported: true,
    };
    f.values.agents_get_endpoint = {
      agent_id: "codex",
      supported: false,
      unsupported_reason: "Synthetic endpoint unavailable",
    };
    f.values.agents_set_model = (args: any) => {
      f.values.agents_get_model.current = args.model;
      return f.values.agents_get_model;
    };
    await f.client.invalidateQueries({ queryKey: ["agents", "list"] });
  });
  await go(page, "AI 代理");
  await page
    .getByRole("button", { name: "查看 Synthetic Codex 详情", exact: true })
    .click();
  const drawer = page.getByRole("dialog", {
    name: "Synthetic Codex 详情",
    exact: true,
  });
  await drawer.getByRole("button", { name: "模型", exact: true }).click();
  await drawer.getByRole("button", { name: /Synthetic B/ }).click();
  await expect(
    drawer.getByRole("button", { name: /Synthetic B/ }),
  ).toBeDisabled();
  await drawer.getByRole("button", { name: "概览", exact: true }).click();
  await drawer.getByRole("button", { name: "模型", exact: true }).click();
  await expect(
    drawer.getByText("synthetic-b", { exact: true }).first(),
  ).toBeVisible();
  expect(await count(page, "agents_get_model")).toBe(1);
  expect(await count(page, "agents_set_model")).toBe(1);
  expect(await count(page, "agents_get_endpoint")).toBe(1);
});

test("Wiki Agent configuration saves in memory, refreshes status and reopens without rereading", async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.wiki_agent_get_config = {
      backend: "claude-code",
      auto_digest: false,
      interval_hours: 6,
      vault_root: "synthetic",
    };
    f.values.wiki_agent_set_config = (args: any) => {
      f.values.wiki_agent_get_config = args.config;
      f.values.wiki_agent_status.backend = args.config.backend;
      return args.config;
    };
  });
  await go(page, "投料口");
  await page
    .getByRole("button", { name: "配置 Backend AI · 自动消化", exact: true })
    .click();
  const dialog = page.getByRole("dialog", {
    name: "Wiki Agent 配置",
    exact: true,
  });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: /Codex CLI/ }).click();
  await dialog.getByRole("button", { name: /保存/ }).click();
  await expect.poll(() => count(page, "wiki_agent_set_config")).toBe(1);
  await expect.poll(() => count(page, "wiki_agent_status")).toBe(2);
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "配置 Backend AI · 自动消化", exact: true })
    .click();
  expect(await count(page, "wiki_agent_get_config")).toBe(1);
});

test("ASR completion loads its result and recent tasks once despite a late poll and duplicate event", async ({
  page,
}) => {
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.asr_task_reconcile = [
      {
        jobId: "synthetic-asr",
        runtime_state: "managed",
        taskDir: "synthetic-task",
        status: "running",
        engineIds: [],
        outputRoot: "synthetic",
      },
    ];
    f.values.asr_task_get = (args: any) =>
      args.jobId
        ? new Promise((resolve) => {
            f.releaseAsr = () =>
              resolve({
                taskDir: "synthetic-task",
                taskMeta: { status: "running" },
                job: null,
                artifacts: [],
              });
          })
        : {
            taskDir: "synthetic-task",
            taskMeta: { status: "completed" },
            job: null,
            artifacts: [],
            preprocessMeta: null,
            runMatrix: null,
            fusionMeta: null,
            summaryMeta: null,
          };
  });
  await go(page, "快捷面板");
  await expect.poll(() => count(page, "asr_task_get")).toBe(1);
  const before = await count(page, "asr_task_list");
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.emit("asr-job", {
      kind: "finished",
      jobId: "synthetic-asr",
      taskDir: "synthetic-task",
      status: "completed",
      message: "Synthetic ASR completed",
    });
    f.releaseAsr();
  });
  await expect(
    page.getByText("任务包已读取，但还没有可查看的结果文件。", { exact: true }),
  ).toBeVisible();
  await expect.poll(() => count(page, "asr_task_list")).toBe(before + 1);
  await page.waitForTimeout(2000);
  expect(await count(page, "asr_task_get")).toBe(2);
  await page.evaluate(() =>
    (window as any).__performance.emit("asr-job", {
      kind: "finished",
      jobId: "synthetic-asr",
      taskDir: "synthetic-task",
      status: "completed",
    }),
  );
  await page.waitForTimeout(150);
  expect(await count(page, "asr_task_get")).toBe(2);
});

test("Wiki Agent completion refreshes status once and invalidates previously visited knowledge", async ({
  page,
}) => {
  await go(page, "知识库");
  await expect.poll(() => count(page, "wiki_list")).toBe(1);
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.wiki_agent_status = {
      pending_count: 1,
      digested_count: 0,
      total_count: 1,
      backend: "claude-code",
      has_agents_md: true,
      vault_root: "synthetic",
    };
    f.values.wiki_agent_run_ingest = {
      task_id: "synthetic-ingest",
      target_count: 1,
      command_preview: "synthetic",
    };
  });
  await go(page, "投料口");
  await page.getByRole("button", { name: "立即消化", exact: true }).click();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          [...(window as any).__performance.listeners.values()].filter(
            (v: any) => v.event === "wiki-agent:event",
          ).length,
      ),
    )
    .toBe(1);
  const before = await count(page, "wiki_agent_status");
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.wiki_agent_status.pending_count = 0;
    f.values.wiki_agent_status.digested_count = 1;
    f.values.wiki_list = [
      {
        rel_path: "synthetic.md",
        absolute_path: "synthetic",
        title: "Newly ingested synthetic note",
        kind: "note",
        modified_ms: 1,
        size: 1,
      },
    ];
    f.emit("wiki-agent:event", {
      kind: "completed",
      task_id: "synthetic-ingest",
      exit_code: 0,
    });
    f.emit("wiki-agent:event", {
      kind: "completed",
      task_id: "synthetic-ingest",
      exit_code: 0,
    });
  });
  await expect.poll(() => count(page, "wiki_agent_status")).toBe(before + 1);
  await go(page, "知识库");
  await expect(
    page.getByRole("button", {
      name: "Newly ingested synthetic note",
      exact: true,
    }),
  ).toBeVisible();
  expect(await count(page, "wiki_list")).toBe(2);
});

test("agent versions and unified shared sections refresh once and clean subscriptions when navigating away", async ({
  page,
}) => {
  await go(page, "AI 代理");
  await expect.poll(() => count(page, "agent_versions_check")).toBe(1);
  await page
    .getByRole("region", { name: "Agent 版本", exact: true })
    .getByRole("button", { name: "重新检查", exact: true })
    .click();
  await expect.poll(() => count(page, "agent_versions_check")).toBe(2);
  expect(await count(page, "shared_skills_list")).toBe(0);
  await go(page, "能力库");
  await page.getByRole("tab", { name: "工具与工作流", exact: true }).click();
  await expect.poll(() => count(page, "shared_skills_list")).toBe(1);
  // These sections expose refresh through title; scope to avoid unrelated controls.
  await page
    .locator("section")
    .filter({ has: page.getByRole("heading", { name: /共享 Skills/ }) })
    .getByRole("button", { name: /刷新/ })
    .click();
  await expect.poll(() => count(page, "shared_skills_list")).toBe(2);
  await go(page, "历史对话");
  expect(
    await page.evaluate(
      () =>
        [...(window as any).__performance.listeners.values()].filter((v: any) =>
          [
            "model-download",
            "asr-job",
            "studio:event",
            "wiki-agent:event",
          ].includes(v.event),
        ).length,
    ),
  ).toBe(0);
});

test("Wiki AI optimization completion updates the open preview without manual refresh", async ({
  page,
}) => {
  await page.evaluate(() => {
    const { values } = (window as any).__performance;
    values.wiki_list = [
      {
        rel_path: "synthetic.md",
        absolute_path: "synthetic",
        title: "Synthetic optimization",
        kind: "note",
        size: 10,
        modified_ms: 1,
      },
    ];
    values.wiki_read = {
      rel_path: "synthetic.md",
      absolute_path: "synthetic",
      exists: true,
      size: 10,
      content: "Before synthetic optimization",
      error: null,
    };
    values.gdnbit_ai_optimize = {
      task_id: "synthetic-optimize",
      command_preview: "synthetic",
    };
  });
  await go(page, "知识库");
  await page
    .getByRole("button", { name: "Synthetic optimization", exact: true })
    .click();
  await expect(
    page.getByText("Before synthetic optimization", { exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "发布到网站", exact: true }).click();
  page.once("dialog", (dialog) => dialog.accept("synthetic focus"));
  await page.getByRole("button", { name: /AI 优化/ }).click();
  await expect.poll(() => count(page, "gdnbit_ai_optimize")).toBe(1);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          [...(window as any).__performance.listeners.values()].filter(
            (v: any) => v.event === "gdnbit:event",
          ).length,
      ),
    )
    .toBe(1);
  await page.evaluate(() => {
    const f = (window as any).__performance;
    f.values.wiki_read.content = "After synthetic optimization";
    f.emit("gdnbit:event", {
      kind: "completed",
      task_id: "synthetic-optimize",
      exit_code: 0,
    });
  });
  await expect(
    page.getByText("After synthetic optimization", { exact: true }),
  ).toBeVisible();
  expect(await count(page, "wiki_read")).toBe(2);
});

test("system and secret read errors recover through their own retry controls", async ({
  page,
}) => {
  const systemReads = [
    ["磁盘容量", "刷新磁盘容量", "读取磁盘信息失败"],
    ["PATH 项目", "刷新 PATH 项目", "读取 PATH 失败"],
    ["关键进程", "刷新关键进程", "读取进程列表失败"],
  ];
  await page.evaluate(() => {
    const f = (window as any).__performance;
    for (const command of [
      "system_disks",
      "system_path_list",
      "system_processes",
      "secrets_list",
    ])
      f.values[command] = () => {
        throw Error("Synthetic read unavailable");
      };
    f.client.removeQueries({ queryKey: ["system"] });
    f.client.removeQueries({ queryKey: ["secrets", "list"] });
  });
  await go(page, "系统");
  for (const [label, refresh, error] of systemReads) {
    const section = page.getByRole("region", { name: label });
    await expect(section.getByText(error, { exact: true })).toBeVisible();
    await expect(section.getByRole("button", { name: refresh, exact: true })).toBeEnabled();
  }
  await page.evaluate(() => {
    const { values } = (window as any).__performance;
    values.system_disks = [
      {
        drive: "X",
        label: "Synthetic disk",
        total_gb: 100,
        free_gb: 70,
        usage_percent: 30,
        warning: false,
      },
    ];
    values.system_path_list = [
      {
        path: "SyntheticPath",
        raw: "SyntheticPath",
        exists: true,
        source: "user",
      },
    ];
    values.system_processes = [
      {
        pid: 1,
        name: "synthetic.exe",
        display_name: "Synthetic process",
        category: "tool",
        cpu_percent: 0,
        memory_mb: 1,
        run_secs: 1,
      },
    ];
  });
  for (const [label, refresh] of systemReads)
    await page
      .getByRole("region", { name: label })
      .getByRole("button", { name: refresh, exact: true })
      .click();
  await expect(
    page.getByRole("region", { name: "磁盘容量" }).getByRole("heading", { name: "X", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("SyntheticPath", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Synthetic process", { exact: true }),
  ).toBeVisible();
  await go(page, "密钥");
  await expect(
    page.getByText("读取密钥列表失败", { exact: true }),
  ).toBeVisible();
  await page.evaluate(() => {
    (window as any).__performance.values.secrets_list = [];
  });
  await page.getByRole("button", { name: "重试", exact: true }).click();
  await expect(
    page.getByText("未读取到任何密钥", { exact: true }),
  ).toBeVisible();
});

test("repeated slow governance refresh is disabled and performs one read per query", async ({
  page,
}) => {
  await go(page, "治理追溯");
  const commands = [
    "governance_audit_recent",
    "governance_snapshots_list",
    "governance_capability_matrix",
    "governance_config_ownership",
    "governance_interface_registry",
  ];
  for (const command of commands)
    await expect.poll(() => count(page, command)).toBe(1);
  await page.evaluate((commands) => {
    const f = (window as any).__performance;
    f.finishRefresh = [];
    for (const c of commands) {
      const value = f.values[c];
      f.values[c] = () =>
        new Promise((resolve) => f.finishRefresh.push(() => resolve(value)));
    }
  }, commands);
  const refresh = page.getByRole("button", { name: "刷新", exact: true });
  await refresh.click();
  await expect(refresh).toBeDisabled();
  for (const command of commands) expect(await count(page, command)).toBe(2);
  await page.evaluate(() =>
    (window as any).__performance.finishRefresh.forEach((finish: () => void) =>
      finish(),
    ),
  );
  await expect(refresh).toBeEnabled();
});
