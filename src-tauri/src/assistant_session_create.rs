//! Stable, caller-known session identity for draft preparation, never model work.
use super::*;

pub fn public_error(error: &str) -> Option<(&'static str, &'static str)> {
    [
        (
            "SESSION_CREATE_BAD_ARGUMENT",
            "会话创建标识无效，请使用原规范UUID；未创建或修改会话",
        ),
        (
            "SESSION_CREATE_IDENTITY_CONFLICT",
            "原会话编号重复，未猜测或覆盖会话",
        ),
        (
            "SESSION_CREATE_SCOPE_MISMATCH",
            "原创建标识不属于请求来源，未切换或覆盖其他会话",
        ),
        (
            "SESSION_CREATE_LIMIT",
            "会话数量达到当前保护上限，未删除或覆盖旧会话",
        ),
    ]
    .into_iter()
    .find(|(code, _)| {
        error
            .strip_prefix(code)
            .is_some_and(|rest| rest.starts_with(':'))
    })
}

pub(super) fn existing(
    state: &State,
    source: &str,
    key: &str,
    id: &str,
) -> Result<Option<SessionMeta>, String> {
    let mut matches = state.sessions.iter().filter(|session| session.id == id);
    let Some(session) = matches.next() else {
        return Ok(None);
    };
    if matches.next().is_some() {
        return Err("SESSION_CREATE_IDENTITY_CONFLICT: 原会话编号重复，未猜测来源".into());
    }
    if session.source != source || session.source_key != key {
        return Err("SESSION_CREATE_SCOPE_MISMATCH: 原创建标识不属于请求来源".into());
    }
    Ok(Some(meta(session)))
}

impl Engine {
    /// A canonical UUID is also the resulting session ID. After an unknown ACK,
    /// callers query/retry this exact ID rather than create another empty chat.
    pub async fn create_session_requested(
        &self,
        request: Option<&str>,
    ) -> Result<SessionMeta, String> {
        let Some(request) = request else {
            return self.create_session().await;
        };
        if !uuid::Uuid::parse_str(request).is_ok_and(|id| id.to_string() == request) {
            return Err("SESSION_CREATE_BAD_ARGUMENT: 创建标识必须是规范UUID，原记录未修改".into());
        }
        self.make_session_fixed("desktop", "desktop", request).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    async fn fixture(
        state: State,
        fail: bool,
    ) -> (tempfile::TempDir, Arc<Engine>, Arc<AtomicUsize>) {
        let root = tempfile::tempdir().unwrap();
        let saves = Arc::new(AtomicUsize::new(0));
        let count = saves.clone();
        let save: Save = Arc::new(move |_| {
            let n = count.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if fail && n == 0 {
                    Err("synthetic storage failure".into())
                } else {
                    Ok(())
                }
            })
        });
        let model: Model =
            Arc::new(|_| Box::pin(async { panic!("Creating a draft must not call a model") }));
        let control: Control =
            Arc::new(|_| Box::pin(async { panic!("Creating a draft must not execute a task") }));
        let engine = Engine::new(state, save, model, control, root.path().join("workspace"))
            .await
            .unwrap();
        (root, engine, saves)
    }
    #[tokio::test]
    async fn known_session_id_concurrent_and_repeated_creation_saves_once_without_tasks() {
        let (_root, e, saves) = fixture(State::default(), false).await;
        let nonce = uuid::Uuid::new_v4().to_string();
        let (a, b, c) = tokio::join!(
            e.create_session_requested(Some(&nonce)),
            e.create_session_requested(Some(&nonce)),
            e.create_session_requested(Some(&nonce))
        );
        assert_eq!(a.unwrap().id, nonce);
        assert_eq!(b.unwrap().id, nonce);
        assert_eq!(c.unwrap().id, nonce);
        let revision = e.state.lock().await.revision;
        assert_eq!(
            e.create_session_requested(Some(&nonce)).await.unwrap().id,
            nonce
        );
        assert_eq!(e.sessions().await.len(), 1);
        assert_eq!(saves.load(Ordering::SeqCst), 1);
        assert_eq!(e.state.lock().await.revision, revision);
        let state = e.state.lock().await;
        assert!(
            state.sessions[0].messages.is_empty()
                && state.sessions[0].turns.is_empty()
                && state.sessions[0].jobs.is_empty()
        );
    }
    #[tokio::test]
    async fn creation_rejects_bad_ids_duplicate_identity_and_other_source_without_writes() {
        let nonce = uuid::Uuid::new_v4().to_string();
        let (_root, e, saves) = fixture(State::default(), false).await;
        for id in ["not-an-id", nonce.to_uppercase().as_str(), ""] {
            assert!(e
                .create_session_requested(Some(id))
                .await
                .err()
                .unwrap()
                .starts_with("SESSION_CREATE_BAD_ARGUMENT:"));
        }
        assert_eq!(saves.load(Ordering::SeqCst), 0);
        let mut state = State::default();
        state
            .sessions
            .push(empty_session("wechat", "other-owner", &nonce));
        let (_root, e, saves) = fixture(state, false).await;
        assert!(e
            .create_session_requested(Some(&nonce))
            .await
            .err()
            .unwrap()
            .starts_with("SESSION_CREATE_SCOPE_MISMATCH:"));
        assert_eq!(saves.load(Ordering::SeqCst), 0);
        {
            let mut state = e.state.lock().await;
            let duplicate = state.sessions[0].clone();
            state.sessions.push(duplicate);
        }
        assert!(e
            .create_session_requested(Some(&nonce))
            .await
            .err()
            .unwrap()
            .starts_with("SESSION_CREATE_IDENTITY_CONFLICT:"));
        assert_eq!(saves.load(Ordering::SeqCst), 0);
    }
    #[tokio::test]
    async fn failed_save_retries_same_identity_and_legacy_creation_keeps_its_behavior() {
        let (_root, e, saves) = fixture(State::default(), true).await;
        let nonce = uuid::Uuid::new_v4().to_string();
        assert!(e.create_session_requested(Some(&nonce)).await.is_err());
        assert!(e.sessions().await.is_empty());
        assert_eq!(
            e.create_session_requested(Some(&nonce)).await.unwrap().id,
            nonce
        );
        assert_eq!(e.sessions().await.len(), 1);
        assert_ne!(e.create_session_requested(None).await.unwrap().id, nonce);
        assert_eq!(e.sessions().await.len(), 2);
        assert_eq!(saves.load(Ordering::SeqCst), 3);
        for code in [
            "SESSION_CREATE_BAD_ARGUMENT",
            "SESSION_CREATE_SCOPE_MISMATCH",
            "SESSION_CREATE_IDENTITY_CONFLICT",
            "SESSION_CREATE_LIMIT",
        ] {
            let (c, m) = public_error(&format!("{code}: private-test-value")).unwrap();
            assert_eq!(c, code);
            assert!(!m.contains("private-test-value"));
        }
    }
}
