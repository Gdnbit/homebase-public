use super::*;

fn query() -> SessionQuery {
    SessionQuery {
        snapshot_id: None,
        query: String::new(),
        source: None,
        offset: 0,
        limit: 20,
    }
}
fn rows(count: usize) -> Vec<SessionMeta> {
    (0..count)
        .map(|n| SessionMeta {
            id: format!("session-{n:05}"),
            title: format!("合成工作 {n}"),
            source: if n % 2 == 0 { "desktop" } else { "wechat" }.into(),
            updated_at: format!("2026-11-18T00:{:02}:00Z", n % 60),
            message_count: n,
            active_turn_id: None,
            queue_depth: 0,
            queue_paused: false,
        })
        .collect()
}
fn captured(
    catalog: &mut Catalog,
    request: &SessionQuery,
    items: Vec<SessionMeta>,
    revision: u64,
    now: Instant,
) -> SessionQuery {
    let mut request = request.clone();
    request.snapshot_id = Some(catalog.capture(&request, items, revision, now).unwrap());
    request
}
#[test]
fn pages_preserve_all_original_metadata_without_duplicates_and_fit_wire_budget() {
    let now = Instant::now();
    let mut catalog = Catalog::default();
    let items = rows(10_000);
    let request = query();
    let mut request = captured(&mut catalog, &request, items, 7, now);
    let mut seen = BTreeSet::new();
    let mut previous: Option<(String, String)> = None;
    loop {
        let page = catalog.page(&request, now).unwrap();
        assert_eq!(page.version, "homebase-session-catalog-v1");
        assert_eq!(page.total, 10_000);
        assert_eq!(page.captured_revision, 7);
        assert!(
            page.items.len() <= request.limit
                && serde_json::to_vec(&page).unwrap().len() <= PAGE_BYTES
        );
        for row in page.items {
            if let Some((date, id)) = &previous {
                assert!(
                    (date.as_str(), std::cmp::Reverse(id.as_str()))
                        >= (row.updated_at.as_str(), std::cmp::Reverse(row.id.as_str()))
                );
            }
            previous = Some((row.updated_at.clone(), row.id.clone()));
            assert!(seen.insert(row.id));
        }
        match page.next_offset {
            None => {
                assert!(page.complete);
                break;
            }
            Some(next) => {
                assert!(!page.complete);
                assert!(next > request.offset);
                request.offset = next;
                request.limit = 40;
            }
        }
    }
    assert_eq!(seen.len(), 10_000);
}
#[test]
fn explicit_empty_filters_and_deterministic_ties_do_not_hide_unknown_sources() {
    let now = Instant::now();
    let mut catalog = Catalog::default();
    let mut data = rows(4);
    for row in &mut data {
        row.updated_at = "same".into();
    }
    data[0].source = "legacy".into();
    data[3].title = "FOCUS 蓝点".into();
    let request = captured(&mut catalog, &query(), data.clone(), 1, now);
    let page = catalog.page(&request, now).unwrap();
    assert_eq!(page.items[0].source, "legacy");
    assert_eq!(page.items[0].id, "session-00000");
    let request = SessionQuery {
        query: "  FOCUS 蓝点  ".into(),
        source: Some(SessionSource::Wechat),
        ..query()
    }
    .validate()
    .unwrap();
    let request = captured(&mut catalog, &request, data.clone(), 1, now);
    let page = catalog.page(&request, now).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].title, "FOCUS 蓝点");
    let request = SessionQuery {
        query: "no-match".into(),
        ..query()
    };
    let request = captured(&mut catalog, &request, data, 1, now);
    let page = catalog.page(&request, now).unwrap();
    assert_eq!(page.total, 0);
    assert!(page.complete && page.items.is_empty() && page.next_offset.is_none());
}
#[test]
fn refresh_reuses_same_revision_but_new_snapshot_never_changes_old_pages() {
    let now = Instant::now();
    let mut catalog = Catalog::default();
    let request = query();
    let old = captured(&mut catalog, &request, rows(80), 1, now);
    assert_eq!(catalog.current(&request, 1, now), old.snapshot_id);
    assert!(catalog.current(&request, 2, now).is_none());
    let new = captured(&mut catalog, &request, rows(81), 2, now);
    assert_ne!(new.snapshot_id, old.snapshot_id);
    let first = catalog.page(&old, now).unwrap();
    let mut next = old.clone();
    next.offset = first.next_offset.unwrap();
    assert_eq!(catalog.page(&next, now).unwrap().total, 80);
    assert_eq!(catalog.page(&new, now).unwrap().total, 81);
}
#[test]
fn continuation_requires_same_filters_and_an_actually_issued_offset() {
    let now = Instant::now();
    let mut catalog = Catalog::default();
    let request = captured(&mut catalog, &query(), rows(80), 1, now);
    for change in [
        SessionQuery {
            query: "changed".into(),
            ..request.clone()
        },
        SessionQuery {
            source: Some(SessionSource::Desktop),
            ..request.clone()
        },
    ] {
        assert!(catalog
            .page(&change, now)
            .err()
            .unwrap()
            .starts_with("SESSION_CATALOG_FILTER_MISMATCH:"));
    }
    for offset in [1, 21, 80, 81] {
        let change = SessionQuery {
            offset,
            ..request.clone()
        };
        assert!(catalog
            .page(&change, now)
            .err()
            .unwrap()
            .starts_with("SESSION_CATALOG_OFFSET_INVALID:"));
    }
    let original = catalog.page(&request, now).unwrap();
    let next = SessionQuery {
        offset: original.next_offset.unwrap(),
        limit: 1,
        ..request
    };
    assert_eq!(catalog.page(&next, now).unwrap().items.len(), 1);
}
#[test]
fn expired_evicted_and_restarted_catalogs_fail_explicitly_instead_of_requerying() {
    let now = Instant::now();
    let mut catalog = Catalog::default();
    let request = captured(&mut catalog, &query(), rows(1), 1, now);
    assert!(Catalog::default()
        .page(&request, now)
        .err()
        .unwrap()
        .contains("SNAPSHOT_EXPIRED"));
    assert!(catalog
        .page(&request, now + TTL)
        .err()
        .unwrap()
        .contains("SNAPSHOT_EXPIRED"));
    let request = captured(&mut catalog, &query(), rows(1), 1, now);
    for revision in 2..=5 {
        captured(&mut catalog, &query(), rows(1), revision, now);
    }
    assert_eq!(catalog.snapshots.len(), SNAPSHOTS);
    assert!(catalog
        .page(&request, now)
        .err()
        .unwrap()
        .contains("SNAPSHOT_EXPIRED"));
    let request = captured(&mut catalog, &query(), rows(1), 9, now);
    for minute in 1..60 {
        catalog
            .page(&request, now + Duration::from_secs(minute * 60))
            .unwrap();
    }
    assert!(catalog
        .page(&request, now + MAX_AGE)
        .err()
        .unwrap()
        .contains("SNAPSHOT_EXPIRED"));
}
#[test]
fn oversized_rows_and_invalid_identity_never_publish_a_partial_snapshot() {
    let now = Instant::now();
    let mut catalog = Catalog::default();
    let request = captured(&mut catalog, &query(), rows(1), 1, now);
    let id = request.snapshot_id.clone();
    let mut duplicate = rows(2);
    duplicate[1].id = duplicate[0].id.clone();
    assert!(catalog
        .capture(&query(), duplicate, 2, now)
        .err()
        .unwrap()
        .contains("INVALID_DATA"));
    let mut large = rows(1);
    large[0].title = "x".repeat(10_000);
    assert!(catalog
        .capture(&query(), large, 2, now)
        .err()
        .unwrap()
        .contains("ENTRY_LIMIT"));
    let mut large = rows(1);
    large[0].title = "x".repeat(SNAPSHOT_BYTES);
    assert!(catalog
        .capture(&query(), large, 2, now)
        .err()
        .unwrap()
        .contains("LIMIT"));
    assert_eq!(catalog.snapshots.len(), 1);
    assert_eq!(
        catalog.page(&request, now).unwrap().snapshot_id,
        id.unwrap()
    );
}
#[test]
fn argument_bounds_and_public_diagnostics_are_explicit_and_never_echo_input() {
    for change in [
        SessionQuery {
            limit: 0,
            ..query()
        },
        SessionQuery {
            limit: 41,
            ..query()
        },
        SessionQuery {
            query: "x".repeat(241),
            ..query()
        },
        SessionQuery {
            query: "private\ninput".into(),
            ..query()
        },
        SessionQuery {
            snapshot_id: Some("fake".into()),
            ..query()
        },
        SessionQuery {
            offset: 1,
            ..query()
        },
    ] {
        let error = change.validate().err().unwrap();
        let (code, message) = public_error(&error).unwrap();
        assert!(code.starts_with("SESSION_CATALOG_"));
        assert!(!message.contains("private"));
    }
    assert!(serde_json::from_value::<SessionQuery>(
        serde_json::json!({"source":"other","query":""})
    )
    .is_err());
    assert!(
        serde_json::from_value::<SessionQuery>(serde_json::json!({"source_key":"injected"}))
            .is_err()
    );
    assert!(public_error("unrelated: private error").is_none());
}

#[test]
fn byte_budget_uses_real_escaped_unicode_json_and_never_skips_the_next_item() {
    let now = Instant::now();
    let mut catalog = Catalog::default();
    let mut data = rows(50);
    for row in &mut data {
        row.title = "中文🙂\"\\".repeat(120);
    }
    let mut request = captured(
        &mut catalog,
        &SessionQuery {
            limit: 40,
            ..query()
        },
        data,
        1,
        now,
    );
    let mut count = 0;
    loop {
        let page = catalog.page(&request, now).unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() <= PAGE_BYTES);
        assert!(page.items.len() < 40 && !page.items.is_empty());
        count += page.items.len();
        if let Some(next) = page.next_offset {
            assert_eq!(next, count);
            request.offset = next;
        } else {
            break;
        }
    }
    assert_eq!(count, 50);
}

#[test]
fn metadata_byte_budget_evicts_snapshots_before_the_count_limit() {
    let now = Instant::now();
    let mut catalog = Catalog::default();
    let mut data = rows(2_000);
    for row in &mut data {
        row.title = "x".repeat(5_000);
    }
    let first = captured(&mut catalog, &query(), data.clone(), 1, now);
    for revision in 2..=4 {
        captured(&mut catalog, &query(), data.clone(), revision, now);
    }
    assert!(catalog.snapshots.len() < SNAPSHOTS);
    assert!(catalog.snapshots.iter().map(|s| s.bytes).sum::<usize>() <= STORE_BYTES);
    assert!(catalog
        .page(&first, now)
        .err()
        .unwrap()
        .contains("SNAPSHOT_EXPIRED"));
}
