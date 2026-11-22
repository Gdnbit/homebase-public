// Control completion and business outcomes are separate; null is not health evidence.
export function validJob(job, withResult = false) {
  return !!job && typeof job === "object" && typeof job.id === "string" && !!job.id &&
    typeof job.result_available === "boolean" &&
    (job.status !== "running" || !job.result_available) &&
    ["running", "succeeded", "failed", "interrupted_unknown", "uncertain"].includes(job.status) &&
    (!withResult || job.status !== "succeeded" || !job.result_available || Object.hasOwn(job, "result"));
}
export function inspectResult(result) {
  if (result == null || (Array.isArray(result) && result.length === 0))
    return { result_state: "empty", notices: ["调用已返回空结果；这不代表所有业务状态正常，请结合命令含义判断是否预期。"] };
  if (Array.isArray(result)) {
    const states = result.map(inspectResult);
    if (states.some(s => ["reported_failure", "partial"].includes(s.result_state))) return { result_state: "partial", notices: ["部分查询项报告失败、未知或不可用，请查看每项结果及原因。"] };
    if (states.some(s => s.result_state === "in_progress")) return { result_state: "in_progress", notices: ["列表中仍有业务任务运行，请按业务任务编号继续查询。"] };
    return { result_state: "returned" };
  }
  if (typeof result !== "object") return { result_state: "returned" };
  if (result.version === "homebase-session-catalog-v1") {
    if(result.next_offset!=null)return {result_state:'partial',notices:['会话目录还有下一页；按原snapshot_id、筛选与实际next_offset续读。摘要不是执行或文件授权，不自动恢复任务。']};
    if(result.total===0&&result.complete===true&&Array.isArray(result.items)&&result.items.length===0)return {result_state:'empty',notices:['当前会话快照没有匹配项；不是查询失败，也不代表其他来源或筛选没有会话。']};
    if(result.complete!==true)return {result_state:'partial',notices:['会话目录读取尚未确认完整；不能把缺少游标当全部读取完毕。']};
    return {result_state:'returned',notices:['仅为固定时刻的会话摘要；任务与操作授权仍核对原接口。']};
  }
  if (result.version === "homebase-send-receipt-v1") {
    const receipt = result.receipt;
    if (result.lookup_state !== "found" || !receipt || typeof receipt.session_id !== "string" || !receipt.session_id || typeof receipt.turn_id !== "string" || !receipt.turn_id || typeof receipt.state !== "string" || !receipt.state)
      return {result_state:"partial",notices:[result.lookup_state==="not_found"?"当前未找到原请求回执，不证明从未执行；保留原编号核对，不自动重发。":"原回执查询结构不完整或状态无法确认；不能据此创建替代任务。"]};
    const state = receipt.state;
    const original = ['queued','running','waiting_jobs','awaiting_confirmation','waiting_input_queue'].includes(state)
      ? {result_state:'in_progress',notices:['原任务尚未结束；继续查询原编号，等待确认时不得自动确认。']}
      : state==='completed' ? {result_state:'returned'}
      : ['failed','disabled','cancelled','canceled'].includes(state) ? {result_state:'reported_failure',notices:['找到原回执，但原任务失败、停用或取消；详查原任务原因。']}
      : {result_state:'partial',notices:['原任务结果未知或状态尚未适配；保留原编号核对，不自动重发。']};
    return {...original,notices:["仅查询原回执，未重新执行；找到回执不代表业务或产物已验收。",...(original.notices??[])]};
  }
  if(typeof result.submission_id==='string'&&['not_found','prepared','unknown','completed','canceled'].includes(result.state)){
    if(result.state==='completed'&&result.file_state==='present')return {result_state:'returned'};
    const reason=result.state==='prepared'?'原收录仅准备，尚未发布；不是运行中的任务。':result.state==='not_found'?'未找到原回执不证明未执行，不能换编号重做。':result.state==='unknown'?'原收录结果未知，只核对原编号，不重复写入。':result.state==='canceled'?'原准备已取消，没有新笔记被发布。':'原收录已完成，但原文件缺失、修改或无法核对；未重建或覆盖。';
    return {result_state:'partial',notices:[reason]};
  }
  if(result.entry?.schema_version===1&&result.entry?.draft&&Object.hasOwn(result,'observation')){
    if(result.observation==='failed')return {result_state:'partial',notices:['源资料草稿仍可读取，但原收录回执核对失败；不是无任务或空草稿，不能重复投递。']};
    if(result.entry.pending){
      const observed=result.submission?inspectResult(result.submission):null;
      return {result_state:'partial',notices:['原收录尚待核对或落定；当前草稿可能是后来输入，不能与原快照混用。',...(observed?.notices??[])]};
    }
    if(result.submission)return inspectResult(result.submission);
    return {result_state:'returned',notices:result.observation==='not_checked'?['已保存源输入，本次未读取历史笔记；不代表新输入已收录。']:['仅返回源草稿，未发布笔记。']};
  }
  if(result.page?.version === "homebase-session-window-v1") {
    if(result.page.total_messages===0&&Array.isArray(result.messages)&&result.messages.length===0&&result.turns?.length===0)return{result_state:"empty",notices:["原会话确实没有消息；不代表查询失败或其他会话为空。"]};
    const notices=[];
    if(result.page.has_more||result.page.end_index<result.page.total_messages)notices.push("这是部分消息窗口；按实际before_message_id读取更早消息，不能当作完整会话。");
    if(result.turns?.some(item=>item.details_omitted))notices.push("任务仅为概况，完整步骤和确认参数须读取原任务详情；不能以省略字段批准操作。");
    return notices.length?{result_state:"partial",notices}:{result_state:"returned"};
  }
  if (result.details_omitted === true && typeof result.id === "string" && typeof result.session_id === "string") {
    const notices = ["这是原任务的进度概况，完整步骤和任务结果请按同一编号查询详情。"];
    if (["queued", "running", "waiting_jobs", "awaiting_confirmation", "waiting_input_queue"].includes(result.state))
      return { result_state: "in_progress", notices };
    if (["failed", "disabled", "canceled"].includes(result.state))
      return { result_state: "reported_failure", notices };
    return { result_state: "partial", notices };
  }
  if(result.version === "homebase-history-v1") {
    const notices=[];
    if(result.next_offset!=null)notices.push(result.format?"历史正文还有分块；按原快照、记录和next_offset续读，拼接后校验JSON哈希。":"历史快照还有下一页；保留原筛选和snapshot_id，按next_offset续读。");
    if(result.omitted_unassigned_messages>0)notices.push("截止轮次之前的归属无法确认的旧消息已省略；这不是完整会话全文。");
    if(!notices.length&&Array.isArray(result.items)&&result.total===0)return {result_state:"empty",notices:["当前会话快照中没有匹配历史；查询已结束，不代表其他会话没有记录。"]};
    if(notices.length)notices.push("读取尚未结束不代表业务任务正在执行；实时进度仍查询原任务，不能重提。");
    return notices.length?{result_state:"partial",notices}:{result_state:"returned"};
  }
  if(result.version === "homebase-capability-discovery-v1") {
    const notices=[];
    if(result.source_complete===false)notices.push("部分能力来源未完整读取；请查看issues并按原来源核对，不将缺项当成空目录。");
    if (result.unmatched_entry_keys?.length)
      notices.push(result.source_complete === false
        ? "部分所选实现尚未核实；查看 unmatched_entry_keys 和来源问题。"
        : "部分所选实现未匹配当前筛选；查看 unmatched_entry_keys。");
    if(result.next_offset!=null)notices.push("能力目录还有下一页；按原snapshot_id、筛选条件及next_offset续读，不代表有执行任务正在运行。");
    if(result.groups_has_more||result.issues_has_more)notices.push("功能分组或来源说明有省略；可缩小筛选并核对原来源。");
    return notices.length?{result_state:"partial",notices}:{result_state:"returned"};
  }
  if (result.success === false || result.ok === false || ["failed", "error", "interrupted", "cancelled", "canceled", "timed_out"].includes(result.status))
    return { result_state: "reported_failure", notices: ["控制调用已结束，但业务结果报告失败或中断，请检查结果中的状态与错误。"] };
  const document = result.extraction ?? result;
  if (document && typeof document === "object" && typeof document.document_id === "string" && typeof document.id === "string" && typeof document.phase === "string") {
    if (["processing", "cancel_requested"].includes(document.phase)) return {result_state:"in_progress",notices:["原文档仍在解析或确认停止；按原 extraction_id 查询，不重新提交。"]};
    if (["failed", "cancelled"].includes(document.phase)) return {result_state:"reported_failure",notices:["本地文档提取失败或已停止，原因见原提取记录；没有可确认的成功文字。"]};
    if (["unknown", "no_text", "awaiting_pages"].includes(document.phase)) return {result_state:"partial",notices:[document.phase==="no_text"?"没有提取到文字；可能是扫描件或空白页，当前解析不支持 OCR。":document.phase==="awaiting_pages"?"需要明确选择至多40个连续页；尚未得到所需文字。":"原文档解析结果待核对，不自动重新解析。"]};
    if(document.phase==="ready"&&(document.partial===true||document.missing_text_pages?.length))return {result_state:"partial",notices:["已取得所选文字，来源仍有未覆盖或没有文字的页；不要将它当作整份文档完整识别。"]};
  }
  if(["capacity", "runtime_unavailable", "not_running_here"].includes(result.status)) return {result_state:"partial",notices:["本次没有启动解析或没有确认停止原进程，详情见 message；不等于文档任务完成。"]};
  if (result.status === "partial")
    return { result_state: "partial", notices: ["调用只完成了部分步骤，请检查 message / warning 及原任务状态，不要盲目重做。"] };
  if (["unmanaged", "unknown"].includes(result.runtime_state))
    return { result_state: "partial", notices: ["任务运行状态未确认，原始记录未修改；不能把记录中的 running 当作仍在运行的证明。"] };
  const notices = [];
  if (!Object.hasOwn(result,"status") && !Object.hasOwn(result,"state") && (typeof result.task_id === "string" || (typeof result.jobId === "string" && typeof result.taskDir === "string")))
    return {result_state:"in_progress",notices:["只返回了业务任务编号，尚未证明业务完成；按 describe 中的 completion 继续查询。"]};
  if (result.history_lost === true || result.truncated === true) notices.push("任务事件已淘汰或截断，不能证明输出完整；禁止自动重跑有副作用任务。");
  if ((result.settled === false && result.status !== "unknown") || result.has_more === true)
    return { result_state: "in_progress", notices: [...notices, "事件输出尚未读完，请用 next_seq 继续查询，直到 settled=true 且 has_more=false。"] };
  const nested = [result.job, result.task_meta].filter(value => value && typeof value === "object").map(inspectResult);
  if (nested.some(outcome => outcome.result_state === "in_progress")) return { result_state: "in_progress", notices: ["业务任务仍在进行，状态见 result.job / result.task_meta；请按业务任务编号继续查询。"] };
  if (nested.some(outcome => outcome.result_state === "reported_failure")) return { result_state: "reported_failure", notices: ["业务任务记录报告失败或中断，详情见 result.job / result.task_meta。"] };
  if (nested.some(outcome => outcome.result_state === "partial")) notices.push("业务任务包含未知状态，详情见 result.job / result.task_meta。");
  if ((Object.hasOwn(result, "current") && result.current == null) || (Object.hasOwn(result, "latest") && result.latest == null)) notices.push("版本信息未知或不可用，请查看 message，不能判定已是最新版本。");
  if (["unknown", "interrupted_unknown", "not_implemented", "skipped", "blocked", "not_configured"].includes(result.status ?? result.stage ?? result.state)) notices.push("业务状态未知、未配置、被阻断或尚未实现，请查看对应原因。");
  for (const key of ["agents", "checks", "results", "jobs"]) {
    if (Array.isArray(result[key]) && result[key].some(item => ["reported_failure", "partial"].includes(inspectResult(item).result_state))) notices.push(`部分 ${key} 项未能完成或无法确认，请查看逐项结果。`);
  }
  if (Array.isArray(result.issues) && result.issues.length) notices.push("部分系统字段未知，原因见 result.issues。");
  if (result.error || result.available === false || result.supported === false) notices.push("结果包含不可用、不支持或错误信息，请查看业务字段。");
  if (Array.isArray(result.buckets) && result.buckets.some(b => b.supported === false)) notices.push("部分数据源不可用，原因见 result.buckets。");
  if (Object.entries(result).some(([key, value]) => key.endsWith("_status") && ["unknown", "invalid", "not_detected", "missing"].includes(value))) notices.push("结果包含未知、未检测到或缺失状态，请查看对应状态说明。");
  if (["running", "queued", "pending", "starting"].includes(result.status ?? result.state))
    return { result_state: "in_progress", notices: [...notices, "业务仍在进行，请按业务任务编号继续查询，控制 job 成功不等于业务完成。"] };
  if (notices.length) return { result_state: "partial", notices };
  return { result_state: "returned" };
}
