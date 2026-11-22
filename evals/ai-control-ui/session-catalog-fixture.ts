/** Synthetic IPC counterpart only. No real files, configuration or transport. */
export function sessionCatalogFixture(getRows: () => Promise<any[]>) {
  const requests: any[] = [];
  const snapshots = new Map<string, any>();
  let revision = 0,
    previous = "";
  return {
    requests,
    snapshots,
    async query({ request }: any) {
      requests.push(structuredClone(request));
      const query = (request.query ?? "").trim().toLowerCase(),
        source = request.source ?? null;
      const offset = request.offset ?? 0,
        limit = request.limit ?? 20;
      if (limit < 1 || limit > 40)
        throw Error("SESSION_CATALOG_ARGUMENT_INVALID: synthetic limit");
      let snapshot = snapshots.get(request.snapshot_id);
      if (request.snapshot_id && !snapshot)
        throw Error(
          "SESSION_CATALOG_SNAPSHOT_EXPIRED: synthetic expired snapshot",
        );
      if (!request.snapshot_id) {
        if (offset)
          throw Error(
            "SESSION_CATALOG_SNAPSHOT_REQUIRED: synthetic missing snapshot",
          );
        const raw = await getRows();
        const rows = raw.map((r) => ({
          id: r.id,
          title: r.title ?? "",
          source: r.source ?? "desktop",
          updated_at: r.updated_at ?? "1970-01-01T00:00:00Z",
          message_count: r.message_count ?? 0,
          active_turn_id: r.active_turn_id ?? null,
          queue_depth: r.queue_depth ?? 0,
          queue_paused: r.queue_paused ?? false,
        }));
        const fingerprint = JSON.stringify(rows);
        if (previous !== fingerprint) {
          previous = fingerprint;
          revision++;
        }
        snapshot = [...snapshots.values()].find(
          (s) =>
            s.revision === revision && s.query === query && s.source === source,
        );
        if (!snapshot) {
          const items = rows
            .filter(
              (r) =>
                (!source || r.source === source) &&
                (!query ||
                  [r.id, r.title, r.source].some((t) =>
                    t.toLowerCase().includes(query),
                  )),
            )
            .sort(
              (a, b) =>
                b.updated_at.localeCompare(a.updated_at) ||
                a.id.localeCompare(b.id),
            );
          snapshot = {
            id: crypto.randomUUID(),
            query,
            source,
            revision,
            items,
            issued: new Set([0]),
            captured_at: new Date().toISOString(),
          };
          if (snapshots.size >= 4)
            snapshots.delete(snapshots.keys().next().value!);
          snapshots.set(snapshot.id, snapshot);
        }
      }
      if (snapshot.query !== query || snapshot.source !== source)
        throw Error(
          "SESSION_CATALOG_FILTER_MISMATCH: synthetic changed filter",
        );
      if (!snapshot.issued.has(offset))
        throw Error("SESSION_CATALOG_OFFSET_INVALID: synthetic guessed page");
      const page = {
        version: "homebase-session-catalog-v1",
        snapshot_id: snapshot.id,
        query,
        source,
        captured_revision: snapshot.revision,
        captured_at: snapshot.captured_at,
        expires_in_seconds: 300,
        total: snapshot.items.length,
        offset,
        next_offset: null as number | null,
        complete: false,
        items: snapshot.items.slice(offset, offset + limit),
      };
      for (;;) {
        const end = offset + page.items.length;
        page.next_offset = end < page.total ? end : null;
        page.complete = page.next_offset === null;
        if (new TextEncoder().encode(JSON.stringify(page)).length <= 9000)
          break;
        if (!page.items.pop())
          throw Error("SESSION_CATALOG_ENTRY_LIMIT: synthetic oversize");
      }
      if (!page.items.length && offset < page.total)
        throw Error("SESSION_CATALOG_ENTRY_LIMIT: synthetic oversize");
      if (page.next_offset !== null) snapshot.issued.add(page.next_offset);
      return structuredClone(page);
    },
  };
}
