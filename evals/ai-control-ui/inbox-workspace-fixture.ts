// Synthetic process store only. No real IPC, account, filesystem or browser storage.
export function inboxWorkspaceFixture(values: Record<string, any>) {
  const seed = (window as any).__inboxSeed;
  const fixture = {
    entry: structuredClone(seed?.entry ?? { schema_version: 1, revision: 0, draft: { text: "", title: "", tags_input: "", source_hint: "", attachments: [], attachment_unconfirmed: false }, pending: null, last_submission_id: null }),
    receipts: new Map<string, any>(Object.entries(seed?.receipts ?? {})),
    notes: [] as any[],
  };
  const same = (a: any, b: any) => JSON.stringify(a) === JSON.stringify(b);
  const view = (id: string) => {
    const r = fixture.receipts.get(id);
    return { submission_id: id, state: r?.state ?? "not_found", file_state: r?.state === "completed" ? "present" : null, result: r?.result ?? null, message: r?.state === "completed" ? "已核对原笔记，不会重新收录。" : "原收录待核对，未自动执行。" };
  };
  const reply = (observe = true) => {
    const id = fixture.entry.pending?.submission_id ?? fixture.entry.last_submission_id;
    return structuredClone({ entry: fixture.entry, submission: observe && id ? view(id) : null, observation: id ? observe ? "loaded" : "not_checked" : "not_needed", observation_error: null });
  };
  const bound = (id: string) => {
    const p = fixture.entry.pending;
    if (!p || p.submission_id !== id) throw Error("INBOX_WORKSPACE_ID");
    const r = fixture.receipts.get(id);
    if (r && !same(r.snapshot, p.snapshot)) throw Error("INBOX_SUBMISSION_CONFLICT");
    return p;
  };
  const settle = (id: string) => {
    const p = bound(id), r = fixture.receipts.get(id);
    if (["completed", "canceled"].includes(r?.state)) {
      if (r.state === "completed") {
        for (const key of ["text", "title", "tags_input"]) if (fixture.entry.draft[key] === p.snapshot[key]) fixture.entry.draft[key] = "";
        fixture.entry.draft.attachments = fixture.entry.draft.attachments.filter((a: any) => !p.snapshot.attachments.some((b: any) => same(a, b)));
        if (!fixture.entry.draft.attachments.length) fixture.entry.draft.attachment_unconfirmed = false;
      }
      fixture.entry.pending = null;
      fixture.entry.last_submission_id = id;
      fixture.entry.revision++;
    }
    return reply();
  };
  Object.assign(values, {
    inbox_workspace_get: () => reply(),
    inbox_workspace_save: ({ request }: any) => {
      if (same(fixture.entry.draft, request.draft)) return reply(false);
      if (fixture.entry.revision !== request.revision) throw Error("INBOX_WORKSPACE_CONFLICT");
      fixture.entry.draft = structuredClone(request.draft); fixture.entry.revision++; return reply(false);
    },
    inbox_workspace_prepare: ({ request }: any) => {
      const old = fixture.entry.pending;
      if (old && (old.submission_id !== request.submission_id || !same(old.snapshot, request.snapshot))) throw Error("INBOX_WORKSPACE_PENDING");
      if (!old) {
        if (request.revision !== fixture.entry.revision) throw Error("INBOX_WORKSPACE_CONFLICT");
        fixture.entry.draft = structuredClone(request.snapshot);
        fixture.entry.pending = { submission_id: request.submission_id, snapshot: structuredClone(request.snapshot), stage: "reserved" };
        fixture.entry.revision++;
      }
      if (old?.stage === "ready") return reply();
      if (!fixture.receipts.has(request.submission_id)) fixture.receipts.set(request.submission_id, { state: "prepared", snapshot: structuredClone(request.snapshot) });
      bound(request.submission_id).stage = "ready"; fixture.entry.revision++; return reply();
    },
    inbox_workspace_submit: async ({ submissionId: id }: any) => {
      const p = bound(id), r = fixture.receipts.get(id);
      if (p.stage !== "ready") throw Error("INBOX_WORKSPACE_NOT_READY");
      if (r?.state === "prepared") {
        const payload = { text: p.snapshot.text, title: p.snapshot.title || undefined, extra_tags: p.snapshot.tags_input.split(/[\s,，]+/).filter(Boolean), source_hint: p.snapshot.source_hint || undefined, attachments: p.snapshot.attachments.map((a: any) => a.rel), submission_id: id };
        const result = values.inbox_submit ? await values.inbox_submit({ payload }) : { path: `F:/synthetic-only/${id}.md`, filename: `${id}.md`, day: "2026-11-17", char_count: payload.text.length };
        fixture.notes.push({ id, payload, result }); r.state = "completed"; r.result = result;
      }
      return settle(id);
    },
    inbox_workspace_resolve: ({ submissionId }: any) => settle(submissionId),
    inbox_workspace_cancel: ({ submissionId }: any) => {
      bound(submissionId); const r = fixture.receipts.get(submissionId);
      if (r?.state !== "prepared") throw Error("INBOX_SUBMISSION_CANCEL_BLOCKED");
      r.state = "canceled"; return settle(submissionId);
    },
  });
  return fixture;
}
