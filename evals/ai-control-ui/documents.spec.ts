import { test, expect, type Page } from "@playwright/test";
import { navigate } from "./navigation";
async function ready(page:Page) {
  await page.route("**/*",r=>new URL(r.request().url()).hostname==="127.0.0.1"?r.continue():r.abort());
  await page.goto("/evals/ai-control-ui/performance.html");
  await page.evaluate(()=>{
    const p=(window as any).__performance;
    p.values.assistant_config_get.settings.enabled=true;
    p.sent=[];p.documents=[];p.uploads=new Map();p.extractions=[];p.docMode="ready";p.docResults=0;
    p.values.assistant_send=({request}:any)=>{p.sent.push(request);return{session_id:request.session_id||"A",turn_id:"doc-turn",state:"completed"};};
    p.values.assistant_document_index=()=>({documents:p.documents,uploads:[...p.uploads.values()].filter((u:any)=>!u.committed),unreadable:0,complete:true});
    p.values.assistant_document_import=({request:r}:any)=>{
      const old=p.uploads.get(r.document_id)||{document_id:r.document_id,scope:r.scope,name:r.name,source_sha256:r.source_sha256,source_bytes:r.source_bytes,parts:Math.ceil(r.source_bytes/(256*1024)),received:[],missing:[],committed:false};
      if(!old.received.includes(r.part))old.received.push(r.part);
      old.missing=Array.from({length:old.parts},(_,i)=>i).filter(i=>!old.received.includes(i));old.complete=!old.missing.length;
      p.uploads.set(r.document_id,old);return old;
    };
    p.values.assistant_document_import_commit=({scope,documentId}:any)=>{const u=p.uploads.get(documentId);u.committed=true;const meta={id:documentId,origin_scope:scope,name:u.name,source_sha256:u.source_sha256,source_bytes:u.source_bytes};p.documents.push(meta);return meta;};
    p.values.assistant_document_get=({documentId}:any)=>({document:p.documents.find((d:any)=>d.id===documentId),extractions:p.extractions.filter((e:any)=>e.document_id===documentId)});
    p.values.assistant_document_extract=({documentId,pages}:any)=>{
      const e={document_id:documentId,id:"a".repeat(64),attempt:"e544c751-b496-4c5b-b8fe-f3c2daf11ce2",revision:1,pages,phase:p.docMode,code:null,message:p.docMode==="no_text"?"所选页没有可提取文字":"原解析已完成",total_pages:2,text_bytes:22,partial:false,missing_text_pages:[]};
      p.extractions=[e];return{status:"started",extraction:e,message:e.message};
    };
    p.values.assistant_document_status=()=>p.extractions[0];
    p.values.assistant_document_result=()=>{p.docResults++;return{extraction:p.extractions[0],error:null,message:"原结果",result:{status:"ready",message:"原结果",page_from:1,page_to:1,total_pages:2,partial:true,missing_text_pages:[],pages:[{number:1,text:"蓝色圆点🙂\n这是合成 PDF 资料。",bytes:50,empty:false}]}};};
    p.values.assistant_document_excerpt=({documentId,extractionId}:any)=>({name:"PDF摘录.txt",content:"蓝色圆点🙂这是合成 PDF 资料。",document:{document_id:documentId,extraction_id:extractionId,source_name:"synthetic.pdf",source_sha256:p.documents[0].source_sha256,page_from:1,page_to:1,text_from:0,text_to:30,content_sha256:"b".repeat(64)}});
    p.values.assistant_document_cancel=()=>{p.extractions[0].phase="cancel_requested";return{status:"cancel_requested",extraction:p.extractions[0],message:"正在确认停止"};};
  });
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await expect(page.getByLabel("发送消息",{exact:true})).toBeEnabled();
}
test.beforeEach(async({page})=>{await ready(page);});
test("WeChat PDF preview reads the original session without uploading or starting extraction", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    const sessionId = "wechat-pdf-preview";
    const documentId = "25b72754-c533-4d89-a874-a2012bea94bf";
    fixture.values.assistant_sessions_list = [
      { id: sessionId, title: "隔离微信来稿", source: "wechat", active_turn_id: null, updated_at: "2026-11-19T02:00:00Z" },
      { id: "wechat-pdf-other", title: "隔离另一会话", source: "wechat", active_turn_id: null, updated_at: "2026-11-19T01:00:00Z" },
    ];
    fixture.values.assistant_session_get = ({ sessionId: id }: { sessionId: string }) => ({ id, source: "wechat", messages: [], turns: [] });
    fixture.documents = [{ id: documentId, origin_scope: sessionId, name: "微信资料.pdf", source_sha256: "b".repeat(64), source_bytes: 1200 }];
    fixture.extractions = [{ document_id: documentId, id: "a".repeat(64), attempt: "original-attempt", revision: 1,
      pages: { from: 1, to: 1 }, phase: "ready", message: "原首页已解析", total_pages: 2 }];
    fixture.documentScopes = [];
    const index = fixture.values.assistant_document_index;
    fixture.values.assistant_document_index = ({ scope }: { scope: string }) => {
      fixture.documentScopes.push(scope);
      return scope === sessionId ? index() : { documents: [], uploads: [], unreadable: 0, complete: true };
    };
  });
  await page.getByRole("button", { name: "刷新助理", exact: true }).click();
  await page.getByRole("link", { name: /隔离微信来稿/ }).click();
  const open = page.getByRole("button", { name: "微信 PDF", exact: true });
  await expect(open).toBeVisible();
  expect(await documentCalls(page, "assistant_document_index")).toBe(0);
  await open.click();
  const dialog = page.getByRole("dialog", { name: "PDF 文字预览" });
  await dialog.getByRole("button", { name: /微信资料\.pdf/ }).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点🙂");
  for (const label of ["选择 PDF", "提取文字", "加入消息", "停止原解析"]) {
    await expect(dialog.getByRole("button", { name: label, exact: true })).toHaveCount(0);
  }
  await dialog.getByRole("button", { name: "关闭 PDF 预览" }).click();
  await page.getByRole("link", { name: /隔离另一会话/ }).click();
  await open.click();
  await expect(dialog.getByText("这个微信会话还没有保存的 PDF。")).toBeVisible();
  await expect(dialog.getByRole("button", { name: /微信资料\.pdf/ })).toHaveCount(0);
  await dialog.getByRole("button", { name: "关闭 PDF 预览" }).click();
  const data = await page.evaluate(() => {
    const fixture = (window as any).__performance;
    return { scopes: fixture.documentScopes, calls: fixture.calls as string[] };
  });
  expect(data.scopes).toEqual(["wechat-pdf-preview", "wechat-pdf-other"]);
  for (const command of ["assistant_document_import", "assistant_document_extract", "assistant_document_cancel", "assistant_send"]) {
    expect(data.calls).not.toContain(command);
  }
});
async function importFile(page:Page) {
  await page.getByLabel("选择文本附件").setInputFiles({name:"synthetic.pdf",mimeType:"application/pdf",buffer:Buffer.from("%PDF-1.7\nSYNTHETIC-UI-ONLY")});
  const dialog=page.getByRole("dialog",{name:"PDF 文字预览"});
  await expect(dialog.getByRole("heading",{name:"synthetic.pdf"})).toBeVisible();
  return dialog;
}
test("a successful PDF attachment clears a previous text error without changing the protected draft",async({page})=>{
  await page.evaluate(()=>{(window as any).__performance.values.assistant_config_get.settings.enabled=false;});
  await page.getByRole("button",{name:"刷新助理",exact:true}).click();
  await page.getByLabel("发送消息",{exact:true}).fill("保留原草稿🙂");
  await page.getByLabel("选择文本附件").setInputFiles({name:"original.md",mimeType:"text/plain",buffer:Buffer.from("原附件，不要覆盖")});
  await expect(page.getByLabel("待发送附件")).toContainText("original.md");
  await page.getByLabel("选择文本附件").setInputFiles({name:"bad.txt",mimeType:"text/plain",buffer:Buffer.from([0xff,0xfe,0xfd])});
  const oldError=page.getByRole("alert").filter({hasText:"文件不是 UTF-8 文本"});
  await expect(oldError).toBeVisible();
  const dialog=await importFile(page);
  await dialog.getByRole("button",{name:"提取文字",exact:true}).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点🙂");
  // Opening and parsing alone do not resolve an input failure.
  await expect(oldError).toBeAttached();
  await page.evaluate(()=>{
    const p=(window as any).__performance,original=p.values.assistant_document_excerpt;
    p.values.assistant_document_excerpt=(args:any)=>{p.excerptAttempts=(p.excerptAttempts||0)+1;if(p.excerptAttempts===1)throw Error("合成摘录失败，请重试");return original(args);};
  });
  await dialog.getByRole("button",{name:"加入消息",exact:true}).click();
  await expect(dialog.getByRole("alert")).toContainText("合成摘录失败");
  await expect(oldError).toBeAttached();
  await expect(page.getByLabel("待发送附件")).not.toContainText("PDF摘录.txt");
  await dialog.getByRole("button",{name:"加入消息",exact:true}).click();
  await expect(dialog).not.toBeVisible();
  await expect(oldError).toHaveCount(0);
  await expect(page.getByLabel("待发送附件")).toContainText("original.md");
  await expect(page.getByLabel("待发送附件")).toContainText("PDF摘录.txt");
  await expect(page.getByLabel("发送消息",{exact:true})).toHaveValue("保留原草稿🙂");
  await expect(page.getByRole("button",{name:"发送",exact:true})).toBeDisabled();
  const state=await page.evaluate(()=>{const p=(window as any).__performance;return{sent:p.sent.length,attempts:p.excerptAttempts,extracts:p.calls.filter((n:string)=>n==="assistant_document_extract").length};});
  expect(state).toEqual({sent:0,attempts:2,extracts:1});
});
test("a rejected PDF attachment keeps the error and can recover without reparsing or replacing files",async({page})=>{
  await page.getByLabel("发送消息",{exact:true}).fill("附件满额也保留原文");
  await page.getByLabel("选择文本附件").setInputFiles([1,2,3,4].map(n=>({name:`keep-${n}.md`,mimeType:"text/plain",buffer:Buffer.from(`保留 ${n}`)})));
  await expect(page.getByLabel("待发送附件").getByRole("listitem")).toHaveCount(4);
  await page.getByLabel("选择文本附件").setInputFiles({name:"bad.txt",mimeType:"text/plain",buffer:Buffer.from([0xff])});
  const oldError=page.getByRole("alert").filter({hasText:"一次最多添加 4 个文本附件"});
  await expect(oldError).toBeVisible();
  const dialog=await importFile(page);
  await dialog.getByRole("button",{name:"提取文字",exact:true}).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点");
  await dialog.getByRole("button",{name:"加入消息",exact:true}).click();
  await expect(dialog.getByRole("alert")).toContainText("4");
  await expect(oldError).toBeAttached();
  await dialog.getByLabel("关闭 PDF 预览").click();
  await page.getByRole("button",{name:"移除 keep-4.md",exact:true}).click();
  await expect(oldError).toBeVisible();
  await page.getByRole("button",{name:"管理 PDF 文档",exact:true}).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点");
  await dialog.getByRole("button",{name:"加入消息",exact:true}).click();
  await expect(dialog).not.toBeVisible();
  await expect(oldError).toHaveCount(0);
  await expect(page.getByLabel("待发送附件").getByRole("listitem")).toHaveCount(4);
  for(const n of [1,2,3]) await expect(page.getByLabel("待发送附件")).toContainText(`keep-${n}.md`);
  await expect(page.getByLabel("待发送附件")).toContainText("PDF摘录.txt");
  await expect(page.getByLabel("发送消息",{exact:true})).toHaveValue("附件满额也保留原文");
  expect(await page.evaluate(()=>(window as any).__performance.sent.length)).toBe(0);
  expect(await page.evaluate(()=>(window as any).__performance.calls.filter((n:string)=>n==="assistant_document_extract").length)).toBe(1);
});
test("PDF import previews locally, explicitly adds provenance and never sends early",async({page})=>{
  await page.getByLabel("发送消息",{exact:true}).fill("保留原有要求");
  const dialog=await importFile(page);
  expect(await page.evaluate(()=>(window as any).__performance.calls.filter((n:string)=>n==="assistant_document_extract").length)).toBe(0);
  await expect(dialog.getByRole("button",{name:"加入消息",exact:true})).toBeDisabled();
  await dialog.getByRole("button",{name:"提取文字",exact:true}).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点🙂");
  await page.screenshot({path:test.info().outputPath("pdf-ready.png")});
  await dialog.getByRole("button",{name:"加入消息",exact:true}).click();
  await expect(dialog).not.toBeVisible();
  await expect(page.getByLabel("待发送附件")).toContainText("PDF摘录.txt");
  await expect(page.getByLabel("发送消息",{exact:true})).toHaveValue("保留原有要求");
  expect(await page.evaluate(()=>(window as any).__performance.sent.length)).toBe(0);
  await page.getByRole("button",{name:"发送",exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>(window as any).__performance.sent.length)).toBe(1);
  expect(await page.evaluate(()=>(window as any).__performance.sent[0].attachments[0].document.source_sha256)).toMatch(/^[a-f0-9]{64}$/);
});
test("opening or reopening PDF records never reparses and immutable text stays cached",async({page})=>{
  const dialog=await importFile(page);await dialog.getByRole("button",{name:"提取文字",exact:true}).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点");
  await dialog.getByLabel("关闭 PDF 预览").click();
  await page.getByRole("button",{name:"管理 PDF 文档"}).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点");
  const calls=await page.evaluate(()=>(window as any).__performance.calls);
  expect(calls.filter((n:string)=>n==="assistant_document_extract")).toHaveLength(1);
  expect(await page.evaluate(()=>(window as any).__performance.docResults)).toBe(1);
  await expect(page.getByLabel("待发送附件")).toHaveCount(0);
});
test("no text is explicit, retry is not automatic and draft survives",async({page})=>{
  await page.evaluate(()=>{(window as any).__performance.docMode="no_text";});
  await page.getByLabel("发送消息",{exact:true}).fill("原草稿");
  const dialog=await importFile(page);await dialog.getByRole("button",{name:"提取文字",exact:true}).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("没有提取到文字");
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("不支持 OCR");
  await expect(dialog.getByRole("button",{name:"加入消息",exact:true})).toBeDisabled();
  expect(await page.evaluate(()=>(window as any).__performance.docResults)).toBe(0);
  await dialog.getByLabel("关闭 PDF 预览").click();await expect(page.getByLabel("发送消息",{exact:true})).toHaveValue("原草稿");
});
test("unknown original parse is queried on reopen without a resubmission",async({page})=>{
  await page.evaluate(()=>{(window as any).__performance.docMode="unknown";});
  const dialog=await importFile(page);await dialog.getByRole("button",{name:"提取文字",exact:true}).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("原结果待核对");
  await dialog.getByLabel("关闭 PDF 预览").click();await page.getByRole("button",{name:"管理 PDF 文档"}).click();
  await expect(dialog.getByRole("button",{name:"核对原编号"})).toBeVisible();
  await dialog.getByRole("button",{name:"核对原编号"}).click();
  expect(await page.evaluate(()=>(window as any).__performance.calls.filter((n:string)=>n==="assistant_document_extract").length)).toBe(1);
});
test("failed or oversized PDF import preserves the message and never starts extraction",async({page})=>{
  await page.getByLabel("发送消息",{exact:true}).fill("不覆盖这条");
  await page.getByLabel("选择文本附件").setInputFiles({name:"oversized.pdf",mimeType:"application/pdf",buffer:Buffer.alloc(4*1024*1024+1,42)});
  const dialog=page.getByRole("dialog",{name:"PDF 文字预览"});await expect(dialog.getByRole("alert")).toContainText("4 MiB");
  await dialog.getByLabel("关闭 PDF 预览").click();await expect(page.getByLabel("发送消息",{exact:true})).toHaveValue("不覆盖这条");
  expect(await page.evaluate(()=>(window as any).__performance.calls.filter((n:string)=>n==="assistant_document_extract").length)).toBe(0);
});
test("PDF modal is keyboard dismissible and fits a narrow dark desktop",async({page})=>{
  await page.setViewportSize({width:760,height:820});await page.emulateMedia({colorScheme:"dark",reducedMotion:"reduce"});
  await page.evaluate(()=>document.documentElement.classList.add("dark"));
  const dialog=await importFile(page);
  const box=await dialog.boundingBox();expect(box!.x).toBeGreaterThanOrEqual(0);expect(box!.x+box!.width).toBeLessThanOrEqual(760);
  expect(await dialog.evaluate(e=>e.scrollWidth<=e.clientWidth+1)).toBe(true);
  await page.screenshot({path:test.info().outputPath("pdf-narrow-dark.png")});
  await page.keyboard.press("Escape");await expect(dialog).not.toBeVisible();
  await expect(page.getByLabel("发送消息",{exact:true})).toBeEnabled();
});

test("lost upload acknowledgement exposes saved chunks and resumes only missing bytes",async({page})=>{
  await page.evaluate(()=>{
    const p=(window as any).__performance,original=p.values.assistant_document_import;p.docPartCalls=[];
    p.values.assistant_document_import=(args:any)=>{p.docPartCalls.push(args.request.part);const value=original(args);if(p.docPartCalls.length===1)throw Error("合成分块回执丢失");return value;};
  });
  const file={name:"resume.pdf",mimeType:"application/pdf",buffer:Buffer.alloc(300000,42)};
  await page.getByLabel("选择文本附件").setInputFiles(file);
  const dialog=page.getByRole("dialog",{name:"PDF 文字预览"});
  await expect(dialog.getByRole("alert")).toContainText("回执丢失");
  await expect(dialog).toContainText("已保存 1/2 块");
  await dialog.getByLabel("选择 PDF 文档").setInputFiles(file);
  await expect(dialog.getByRole("heading",{name:"resume.pdf"})).toBeVisible();
  expect(await page.evaluate(()=>(window as any).__performance.docPartCalls)).toEqual([0,1]);
  expect(await page.evaluate(()=>(window as any).__performance.documents.length)).toBe(1);
  expect(await page.evaluate(()=>(window as any).__performance.calls.filter((v:string)=>v==="assistant_document_extract").length)).toBe(0);
});

test("reopening during a delayed PDF chunk preserves the same upload and resumes only the missing chunk", async ({ page }) => {
  await page.evaluate(() => {
    const fixture = (window as any).__performance;
    const writePart = fixture.values.assistant_document_import;
    fixture.partsWritten = [];
    fixture.values.assistant_document_import = (args: any) => {
      fixture.partsWritten.push([args.request.document_id, args.request.part]);
      if (fixture.partsWritten.length > 1) return writePart(args);
      return new Promise((resolve) => {
        fixture.finishFirstPart = () => resolve(writePart(args));
      });
    };
  });
  const file = { name: "late-chunk.pdf", mimeType: "application/pdf", buffer: Buffer.alloc(300000, 42) };
  await page.getByLabel("选择文本附件").setInputFiles(file);
  const dialog = page.getByRole("dialog", { name: "PDF 文字预览" });
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.finishFirstPart)).toBe("function");
  await dialog.getByLabel("关闭 PDF 预览").click();
  await page.getByRole("button", { name: "管理 PDF 文档", exact: true }).click();
  await expect(dialog.getByRole("button", { name: "选择 PDF", exact: true })).toBeDisabled();
  const beforeLatePart = await documentCalls(page, "assistant_document_index");
  await page.evaluate(() => (window as any).__performance.finishFirstPart());
  await expect(dialog).toContainText("已保存 1/2 块");
  await expect(dialog.getByRole("button", { name: "选择 PDF", exact: true })).toBeEnabled();
  expect(await documentCalls(page, "assistant_document_index")).toBe(beforeLatePart + 1);
  await dialog.getByLabel("选择 PDF 文档").setInputFiles(file);
  await expect(dialog.getByRole("heading", { name: file.name })).toBeVisible();
  const parts = await page.evaluate(() => (window as any).__performance.partsWritten);
  expect(parts).toHaveLength(2);
  expect(parts[0][1]).toBe(0);
  expect(parts[1]).toEqual([parts[0][0], 1]);
  expect(await documentCalls(page, "assistant_document_import_commit")).toBe(1);
  expect(await documentCalls(page, "assistant_document_extract", "assistant_send")).toBe(0);
});

for (const acknowledgement of ["ready", "lost"]) {
  test(`a delayed PDF commit with ${acknowledgement} acknowledgement refreshes its original index after navigation`, async ({ page }) => {
    await page.evaluate((ack) => {
      const fixture = (window as any).__performance;
      const commit = fixture.values.assistant_document_import_commit;
      const index = fixture.values.assistant_document_index;
      fixture.values.assistant_document_import_commit = (args: any) => new Promise((resolve, reject) => {
        fixture.finishCommit = () => {
          const document = commit(args);
          if (ack === "lost") reject(Error("合成提交回执丢失"));
          else resolve(document);
        };
      });
      fixture.values.assistant_document_index = () => {
        const snapshot = structuredClone(index());
        if (!fixture.holdOldIndex) return snapshot;
        fixture.holdOldIndex = false;
        return new Promise((resolve) => {
          fixture.finishOldIndex = () => resolve(snapshot);
        });
      };
    }, acknowledgement);
    const file = { name: "late-commit.pdf", mimeType: "application/pdf", buffer: Buffer.from("%PDF-1.7\nSYNTHETIC-COMMIT") };
    const dialog = page.getByRole("dialog", { name: "PDF 文字预览" });
    await page.getByLabel("选择文本附件").setInputFiles(file);
    await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.finishCommit)).toBe("function");
    await dialog.getByLabel("关闭 PDF 预览").click();
    await page.getByRole("link", { name: "能力库", exact: true }).click();
    await expect(page.getByRole("heading", { name: "能力库", exact: true })).toBeVisible();
    await page.getByRole("link", { name: "AI 助理", exact: true }).click();
    await page.getByRole("button", { name: "管理 PDF 文档", exact: true }).click();
    if (acknowledgement === "lost") {
      await dialog.getByLabel("选择 PDF 文档").setInputFiles(file);
      await expect(dialog.getByRole("alert")).toContainText("上一份 PDF 仍在保存");
      expect(await documentCalls(page, "assistant_document_import_commit")).toBe(1);
    }
    const refresh = dialog.getByRole("button", { name: "刷新文档列表" });
    await expect(refresh).toBeEnabled();
    await page.evaluate(() => { (window as any).__performance.holdOldIndex = true; });
    await refresh.click();
    await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.finishOldIndex)).toBe("function");
    await page.evaluate(() => (window as any).__performance.finishCommit());
    const item = dialog.getByRole("button", { name: /late-commit\.pdf/ });
    await expect(item).toBeVisible();
    await page.evaluate(() => (window as any).__performance.finishOldIndex());
    await expect(item).toHaveAttribute("aria-pressed", "false");
    await expect(dialog.getByRole("heading", { name: file.name })).toHaveCount(0);
    expect(await documentCalls(page, "assistant_document_import_commit")).toBe(1);
    expect(await documentCalls(page, "assistant_document_extract", "assistant_send")).toBe(0);
  });
}

test("a late excerpt after closing cannot replace later input or add an attachment",async({page})=>{
  const dialog=await importFile(page);await dialog.getByRole("button",{name:"提取文字",exact:true}).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点");
  await page.evaluate(()=>{const p=(window as any).__performance,original=p.values.assistant_document_excerpt;p.values.assistant_document_excerpt=(args:any)=>new Promise(resolve=>{p.releaseExcerpt=()=>resolve(original(args));});});
  await dialog.getByRole("button",{name:"加入消息",exact:true}).click();
  await expect.poll(()=>page.evaluate(()=>typeof (window as any).__performance.releaseExcerpt)).toBe("function");
  await page.keyboard.press("Escape");await expect(dialog).not.toBeVisible();
  await page.getByLabel("发送消息",{exact:true}).fill("关闭后写的新要求");await page.evaluate(()=>(window as any).__performance.releaseExcerpt());
  await expect(page.getByLabel("发送消息",{exact:true})).toHaveValue("关闭后写的新要求");await expect(page.getByLabel("待发送附件")).toHaveCount(0);
});

for (const scenario of ["reopen while waiting", "lost receipt while closed", "return to the conversation"] as const) {
  test(`PDF extraction settlement refreshes the original document after ${scenario}`, async ({ page }) => {
    await page.getByLabel("发送消息", { exact: true }).fill("保留原来的 PDF 提问草稿");
    const dialog = await importFile(page);
    const lost = scenario === "lost receipt while closed";
    await page.evaluate((loseReceipt) => {
      const p = (window as any).__performance;
      const extract = p.values.assistant_document_extract;
      p.values.assistant_document_extract = async (args: unknown) => {
        await new Promise<void>((resolve) => { p.releaseExtraction = resolve; });
        const reply = extract(args);
        if (loseReceipt) throw Error("合成提取回执丢失");
        return reply;
      };
    }, lost);
    const extract = dialog.getByRole("button", { name: "提取文字", exact: true });
    await extract.click();
    await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releaseExtraction)).toBe("function");
    await dialog.getByRole("button", { name: "关闭 PDF 预览" }).click();
    await expect(dialog).not.toBeVisible();
    if (scenario === "return to the conversation") {
      await navigate(page, "工作台");
      await navigate(page, "AI 助理");
      await page.evaluate(() => {
        const p = (window as any).__performance;
        const get = p.values.assistant_document_get;
        const key = ["assistant-documents", "new", p.documents[0].id];
        p.values.assistant_document_get = (args: unknown) => {
          const snapshot = structuredClone(get(args));
          p.values.assistant_document_get = get;
          return new Promise((resolve) => { p.releasePreWriteRead = () => resolve(snapshot); });
        };
        return p.client.invalidateQueries({ queryKey: key, exact: true, refetchType: "none" });
      });
    }
    const open = async () => {
      await page.getByRole("button", { name: "管理 PDF 文档", exact: true }).click();
      const row = dialog.getByRole("button", { name: /synthetic\.pdf/ });
      if ((await row.getAttribute("aria-pressed")) !== "true") await row.click();
      await expect(dialog.getByRole("heading", { name: "synthetic.pdf" })).toBeVisible();
    };
    if (!lost) {
      await open();
      await expect(dialog.getByRole("status").filter({ hasText: "正在核对原提取操作" })).toBeVisible();
      await expect(extract).toBeDisabled();
    }
    if (scenario === "return to the conversation") {
      await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releasePreWriteRead)).toBe("function");
    }
    await page.evaluate(() => (window as any).__performance.releaseExtraction());
    if (lost) {
      await expect.poll(() => page.evaluate(() => (window as any).__performance.extractions.length)).toBe(1);
      await open();
    }
    await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点🙂");
    if (scenario === "return to the conversation") {
      await page.evaluate(() => (window as any).__performance.releasePreWriteRead());
    }
    await expect(dialog.getByLabel("PDF 提取记录")).toHaveValue("a".repeat(64));
    await dialog.getByRole("button", { name: "关闭 PDF 预览" }).click();
    await expect(page.getByLabel("发送消息", { exact: true })).toHaveValue("保留原来的 PDF 提问草稿");
    await expect(page.getByLabel("待发送附件")).toHaveCount(0);
    expect(await documentCalls(page, "assistant_document_extract")).toBe(1);
    expect(await documentCalls(page, "assistant_document_excerpt", "assistant_send")).toBe(0);
  });
}

test("active document polling updates completion once and then stops",async({page})=>{
  await page.evaluate(()=>{(window as any).__performance.docMode="processing";});
  const dialog=await importFile(page);await dialog.getByRole("button",{name:"提取文字",exact:true}).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("正在提取");
  await page.evaluate(()=>{(window as any).__performance.extractions[0].phase="ready";});
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点");
  const before=await page.evaluate(()=>(window as any).__performance.calls.filter((n:string)=>n==="assistant_document_status").length);
  await page.waitForTimeout(1800);
  expect(await page.evaluate(()=>(window as any).__performance.calls.filter((n:string)=>n==="assistant_document_status").length)).toBe(before);
  expect(await page.evaluate(()=>(window as any).__performance.docResults)).toBe(1);
});

const documentCalls = (page: Page, ...commands: string[]) =>
  page.evaluate((names) => {
    const calls: string[] = (window as any).__performance.calls;
    return calls.filter((call) => names.includes(call)).length;
  }, commands);

test("PDF recovery pauses polling on status failure and checks the original receipt once", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.docMode = "processing";
    p.statusUnavailable = true;
    p.checkedReceipts = [];
    p.values.assistant_document_status = (args: any) => {
      p.checkedReceipts.push(args.extractionId);
      if (p.statusUnavailable) throw Error("合成状态读取失败");
      return structuredClone(p.extractions[0]);
    };
  });
  const dialog = await importFile(page);
  await dialog.getByRole("button", { name: "提取文字", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText("合成状态读取失败");
  const before = await documentCalls(page, "assistant_document_status");
  await page.waitForTimeout(1800);
  expect(await documentCalls(page, "assistant_document_status")).toBe(before);
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("原提取状态待核对");
  await expect(dialog.getByRole("button", { name: "停止原解析" })).toBeDisabled();
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.statusUnavailable = false;
    p.extractions[0] = { ...p.extractions[0], phase: "ready", revision: 2 };
  });
  await dialog.getByRole("button", { name: "重新查询" }).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点");
  expect(await documentCalls(page, "assistant_document_status")).toBe(before + 1);
  expect(await documentCalls(page, "assistant_document_extract", "assistant_document_result")).toBe(2);
  expect(await page.evaluate(() => [...new Set((window as any).__performance.checkedReceipts)])).toEqual(["a".repeat(64)]);
});

test("PDF recovery retries the original page range after the draft range changes", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const extract = p.values.assistant_document_extract;
    p.docMode = "failed";
    p.parseRanges = [];
    p.values.assistant_document_extract = (args: any) => {
      p.parseRanges.push([args.pages, args.retry]);
      return extract(args);
    };
  });
  const dialog = await importFile(page);
  await dialog.getByLabel("PDF 起始页").fill("2");
  await dialog.getByLabel("PDF 结束页").fill("2");
  await dialog.getByRole("button", { name: "提取文字", exact: true }).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("提取失败");
  await dialog.getByLabel("PDF 起始页").fill("1");
  await dialog.getByLabel("PDF 结束页").fill("1");
  await dialog.getByRole("button", { name: "显式重试本地提取" }).click();
  expect(await page.evaluate(() => (window as any).__performance.parseRanges)).toEqual([
    [{ from: 2, to: 2 }, false], [{ from: 2, to: 2 }, true],
  ]);
  await expect(dialog.getByLabel("PDF 起始页")).toHaveValue("1");
});

test("PDF recovery ignores a late processing check after the original cancellation receipt", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.docMode = "processing";
    p.values.assistant_document_status = () => {
      const value = structuredClone(p.extractions[0]);
      if (!p.holdCheck) return value;
      p.holdCheck = false;
      return new Promise((resolve) => { p.releaseCheck = () => resolve(value); });
    };
    p.values.assistant_document_cancel = ({ extractionId }: any) => {
      const previous = p.extractions[0];
      if (extractionId !== previous.id) throw Error("Unexpected extraction identity");
      const updated = { ...previous, phase: "cancel_requested", revision: 2, message: "等待原解析停止" };
      p.extractions[0] = updated;
      return { status: "cancel_requested", extraction: updated, message: updated.message };
    };
  });
  const dialog = await importFile(page);
  const text = dialog.getByLabel("PDF 提取文字");
  await dialog.getByRole("button", { name: "提取文字", exact: true }).click();
  await expect(text).toContainText("正在提取");
  await page.evaluate(() => { (window as any).__performance.holdCheck = true; });
  await expect.poll(() => page.evaluate(() => typeof (window as any).__performance.releaseCheck)).toBe("function");
  await dialog.getByRole("button", { name: "停止原解析" }).click();
  await expect(text).toContainText("正在确认停止");
  await page.evaluate(() => (window as any).__performance.releaseCheck());
  await page.waitForTimeout(200);
  expect(await dialog.getByRole("button", { name: "停止原解析" }).isDisabled()).toBe(true);
  expect(await text.textContent()).toContain("正在确认停止");
  await page.evaluate(() => {
    const p = (window as any).__performance;
    p.extractions[0] = { ...p.extractions[0], phase: "cancelled", revision: 3 };
  });
  await expect(text).toContainText("已停止");
  expect(await documentCalls(page, "assistant_document_extract", "assistant_document_cancel")).toBe(2);
});

test("PDF recovery rereads only the failed result and refreshes only the document list", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const result = p.values.assistant_document_result;
    p.values.assistant_document_result = (args: any) => {
      if (!p.allowResult) throw Error("合成文字结果读取失败");
      return result(args);
    };
  });
  const dialog = await importFile(page);
  await dialog.getByRole("button", { name: "提取文字", exact: true }).click();
  await expect(dialog.getByRole("alert")).toContainText("合成文字结果读取失败");
  const statusCalls = await documentCalls(page, "assistant_document_status");
  await page.evaluate(() => { (window as any).__performance.allowResult = true; });
  await dialog.getByRole("button", { name: "重新查询" }).click();
  await expect(dialog.getByLabel("PDF 提取文字")).toContainText("蓝色圆点");
  expect(await documentCalls(page, "assistant_document_status")).toBe(statusCalls);
  expect(await documentCalls(page, "assistant_document_result")).toBe(2);
  const listCalls = await documentCalls(page, "assistant_document_index");
  const detailCommands = ["assistant_document_get", "assistant_document_status", "assistant_document_result"];
  const detailCalls = await documentCalls(page, ...detailCommands);
  await dialog.getByRole("button", { name: "刷新文档列表" }).click();
  await expect.poll(() => documentCalls(page, "assistant_document_index")).toBe(listCalls + 1);
  await expect(dialog.getByRole("button", { name: "刷新文档列表" })).toBeEnabled();
  expect(await documentCalls(page, ...detailCommands)).toBe(detailCalls);
  expect(await documentCalls(page, "assistant_document_extract")).toBe(1);
});

test("PDF recovery waits for the selected document before starting an extraction", async ({ page }) => {
  await page.evaluate(() => {
    const p = (window as any).__performance;
    const getDocument = p.values.assistant_document_get;
    p.values.assistant_document_get = (args: any) => {
      if (!p.allowDocument) throw Error("合成文档详情读取失败");
      return getDocument(args);
    };
  });
  await page.getByLabel("选择文本附件").setInputFiles({
    name: "recovery.pdf",
    mimeType: "application/pdf",
    buffer: Buffer.from("%PDF-1.7\nSYNTHETIC-RECOVERY-ONLY"),
  });
  const dialog = page.getByRole("dialog", { name: "PDF 文字预览" });
  const extract = dialog.getByRole("button", { name: "提取文字", exact: true });
  await expect(dialog.getByRole("alert")).toContainText("合成文档详情读取失败");
  await expect(extract).toBeDisabled();
  await page.evaluate(() => { (window as any).__performance.allowDocument = true; });
  await dialog.getByRole("button", { name: "重新读取文档" }).click();
  await expect(dialog.getByRole("heading", { name: "recovery.pdf" })).toBeVisible();
  await expect(extract).toBeEnabled();
  expect(await documentCalls(page, "assistant_document_get")).toBe(2);
  expect(await documentCalls(page, "assistant_document_extract")).toBe(0);
});
