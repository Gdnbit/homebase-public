import { expect, type Page } from "@playwright/test";

// Exercise the current SPA navigation. Avoid goto/reload: those would discard
// the query cache and invalidate the page-switching assertions in these tests.
export async function navigate(page: Page, label: string) {
  const current: Record<string, string> = {
    "概览": "工作台", "AI 代理": "Agent 管理", "知识库": "资料库", "密钥": "密钥与连接",
  };
  const legacy: Record<string, [string, string?]> = {
    "历史对话": ["原历史时间线", "已有工作入口"],
    "文章生成": ["已有文章与预览", "已有工作入口"],
    "快捷面板": ["快捷面板与已有任务", "已有工作入口"],
    "投料口": ["已有投料记录", "已有工作入口"],
    "系统": ["本机环境"], "治理追溯": ["操作记录"],
  };
  const target = page.getByRole("link", {name: legacy[label] ? "设置" : current[label] ?? label, exact: true});
  if (!await target.isVisible()) {
    const details = page.locator("details").filter({has:page.getByText("更多功能", {exact:true})});
    if (await details.count() && await details.getAttribute("open") === null)
      await details.locator("summary").click();
  }
  await target.click();
  if (legacy[label]) await expect(page.getByRole("heading",{name:"设置",level:1,exact:true})).toBeVisible();
  if (legacy[label]) {
    const [name, section] = legacy[label];
    if (section) {
      const details = page.locator("details").filter({has:page.getByText(section, {exact:true})});
      if (await details.getAttribute("open") === null) await details.locator("summary").click();
    }
    await page.getByRole("link", {name, exact:!!section}).click();
  }
  const headings:Record<string,string>={"概览":"工作台","工作台":"工作台","AI 代理":"AI 代理","Agent 管理":"AI 代理","知识库":"知识库","资料库":"知识库","密钥":"密钥","密钥与连接":"密钥","能力库":"能力库","历史对话":"历史对话","文章生成":"文章生成","投料口":"投料口","系统":"系统","治理追溯":"治理与追溯","设置":"设置"};
  if(headings[label])await expect(page.getByRole("heading",{name:headings[label],level:1,exact:true})).toBeVisible();
  if(label==="快捷面板")await expect(page.getByRole("button",{name:"重新检查下载任务状态",exact:true})).toBeVisible();
}
