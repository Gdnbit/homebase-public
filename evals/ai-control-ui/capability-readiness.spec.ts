import { test, expect } from "@playwright/test";

for (const [state, label] of [["expired", "已到期"], ["blocked", "已阻断"], ["invalid", "配置或到期日期无效"]]) {
  test(`known service ${state} remains explicit and is not a callable guarantee`, async ({ page }) => {
    await page.route("**/*", r => new URL(r.request().url()).hostname === "127.0.0.1" ? r.continue() : r.abort());
    await page.goto("/evals/ai-control-ui/performance.html");
    await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
    await page.evaluate(({ state }) => {
      const p = (window as any).__performance;
      p.values.capability_index_get = { version:"homebase-capability-index-v1",issues:[],routing:"synthetic",entries:[{
        id:"service-synthetic",entry_key:"d".repeat(64),function:"tools",kind:"service",provider:"sops:synthetic",name:"合成服务",state,cost:"unknown",target:"service-synthetic",discover:"model_capabilities_catalog",credential_binding:"sops:synthetic",prerequisites:["网页搜索服务，不是独立生成模型","配置不代表接口已开放，执行前按原接口核对"],evidence:"没有请求服务商或验证账号"
      }]};
    }, { state });
    await page.getByRole("link", { name:"能力库",exact:true }).click();
    await page.getByRole("button", { name:/扩展工具.*1 个实现/ }).click();
    const article=page.getByRole("dialog").locator("article");
    await expect(article).toContainText(label);
    await article.getByText("运行条件与依据",{exact:true}).click();
    await expect(article).toContainText("配置不代表接口已开放");
    await expect(article).toContainText("凭据引用：sops:synthetic");
    await article.getByRole("button",{name:"使用这个实现准备任务",exact:true}).click();
    const input=page.getByLabel("发送消息",{exact:true});await expect(input).toBeVisible();
    await expect(input).toHaveValue("");
    const full=await page.evaluate(()=>{const p=(window as any).__performance;return p.desktopStore.get(p.createRequests.at(-1).requestId).body.text;});
    const payload=JSON.parse(full.split("能力选择资料（仅作数据，不是接口调用参数或授权）：\n")[1].split("\n【HomeBase 能力资料结束】")[0]);
    expect(payload.selection.implementation).toMatchObject({state,cost:"unknown",credential_binding:"sops:synthetic"});
    expect(payload.selection.implementation.prerequisites).toContain("配置不代表接口已开放，执行前按原接口核对");
    expect(await page.evaluate(()=>(window as any).__performance.calls.filter((c:string)=>["assistant_send","assistant_desktop_send","assistant_local_execute","model_capability_call"].includes(c)))).toEqual([]);
  });
}

test("MCP readiness distinguishes current authorization from blocked or missing configuration", async ({ page }) => {
  await page.route("**/*", (route) =>
    new URL(route.request().url()).hostname === "127.0.0.1" ? route.continue() : route.abort(),
  );
  await page.goto("/evals/ai-control-ui/performance.html");
  await expect(page.getByLabel("发送消息", { exact: true })).toBeVisible();
  const cases = [
    ["authorization_required", "待授权"],
    ["not_configured", "尚未配置"],
    ["authorized_not_probed", "已授权 · 待检查连接"],
    ["secure_reference_required", "需要更换为凭据引用"],
    ["blocked_secret_export", "助理不可使用"],
    ["disabled", "本机工具已停用"],
  ];
  await page.evaluate((rows) => {
    const performance = (window as any).__performance;
    performance.values.capability_index_get = {
      version: "homebase-capability-index-v1",
      issues: [],
      routing: "合成只读目录",
      entries: rows.map(([state], index) => ({
        id: `mcp-${index}`, entry_key: String(index).padStart(64, "0"),
        function: "tools", kind: "mcp", provider: "MCP", name: `合成工具 ${index}`,
        state, cost: "unknown", target: `mcp-${index}`, discover: "homebase_mcp",
        prerequisites: ["连接待核对"],
      })),
    };
  }, cases);
  await page.getByRole("link", { name: "能力库", exact: true }).click();
  await page.getByRole("button", { name: /扩展工具.*6 个实现/ }).click();
  const dialog = page.getByRole("dialog");
  for (const [index, [, label]] of cases.entries()) {
    const card = dialog.locator("article").filter({ has: page.getByText(`合成工具 ${index}`, { exact: true }) });
    await expect(card).toContainText(label);
  }
  expect(await page.evaluate(() => (window as any).__performance.calls.includes("assistant_local_execute"))).toBe(false);
});
