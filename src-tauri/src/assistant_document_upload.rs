//! Immutable encrypted upload chunks keep every control request below 1MiB.
use super::*;
pub const CHUNK_BYTES: usize = 256 * 1024;
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Header {
    schema: u32,
    id: String,
    scope: String,
    name: String,
    source_sha256: String,
    source_bytes: usize,
    parts: usize,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct UploadPart {
    pub scope: String,
    pub document_id: String,
    pub name: String,
    pub source_sha256: String,
    pub source_bytes: usize,
    pub part: usize,
    pub data_base64: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunk {
    header: Header,
    part: usize,
    data_base64: String,
}
#[derive(Clone, Serialize)]
pub struct UploadStatus {
    pub document_id: String,
    pub scope: String,
    pub name: String,
    pub source_sha256: String,
    pub source_bytes: usize,
    pub parts: usize,
    pub received: Vec<usize>,
    pub missing: Vec<usize>,
    pub bytes_received: usize,
    pub complete: bool,
    pub committed: bool,
}
impl DocumentStore {
    fn upload_header(&self, id: &str) -> Result<Header, String> {
        let header: Header = self.read(&self.dir(id)?.join("upload.dpapi"), 16 * 1024)?;
        scope(&header.scope)?;
        Self::name(&header.name)?;
        if header.schema != 1
            || header.id != id
            || header.source_bytes == 0
            || header.source_bytes > SOURCE_LIMIT
            || header.parts != header.source_bytes.div_ceil(CHUNK_BYTES)
            || !Self::valid_hash(&header.source_sha256)
        {
            return Err("文档导入记录格式或来源无效，未覆盖".into());
        }
        Ok(header)
    }
    pub(super) fn pending_upload(&self, id: &str) -> bool {
        self.upload_header(id).is_ok() && self.dir(id).is_ok_and(|p| !p.join("meta.dpapi").exists())
    }
    pub fn upload_index(&self, owner: &str) -> Result<Vec<UploadStatus>, String> {
        scope(owner)?;
        let entries = match fs::read_dir(&self.root) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err("文档导入目录不可读".into()),
        };
        let mut result = Vec::new();
        let mut count = 0;
        for entry in entries {
            let entry = entry.map_err(|_| "文档导入条目不可读")?;
            if !entry
                .file_type()
                .map_err(|_| "文档导入条目类型无效")?
                .is_dir()
            {
                continue;
            }
            count += 1;
            if count > MAX_DOCUMENTS {
                return Err("文档导入目录超过上限，未截断".into());
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if entry.path().join("meta.dpapi").exists() {
                continue;
            }
            if let Ok(header) = self.upload_header(&id) {
                if header.scope == owner {
                    result.push(self.upload_status(&id, owner)?);
                }
            }
        }
        Ok(result)
    }
    pub fn upload_status(&self, id: &str, owner: &str) -> Result<UploadStatus, String> {
        let header = self.upload_header(id)?;
        if header.scope != owner {
            return Err("未读取其他输入的文档导入记录".into());
        }
        let dir = self.dir(id)?;
        let mut received = Vec::new();
        let mut missing = Vec::new();
        let mut bytes = 0;
        for part in 0..header.parts {
            if dir.join(format!("upload-{part:02}.dpapi")).is_file() {
                received.push(part);
                bytes += (header.source_bytes - part * CHUNK_BYTES).min(CHUNK_BYTES);
            } else {
                missing.push(part);
            }
        }
        let committed = if dir.join("meta.dpapi").exists() {
            let meta = self.metadata(id)?;
            if meta.origin_scope != header.scope
                || meta.source_sha256 != header.source_sha256
                || meta.name != header.name
                || meta.source_bytes != header.source_bytes
            {
                return Err("导入记录与已提交来源冲突".into());
            }
            true
        } else {
            false
        };
        Ok(UploadStatus {
            document_id: header.id,
            scope: header.scope,
            name: header.name,
            source_sha256: header.source_sha256,
            source_bytes: header.source_bytes,
            parts: header.parts,
            complete: missing.is_empty(),
            received,
            missing,
            bytes_received: bytes,
            committed,
        })
    }
    pub async fn upload_part(&self, request: UploadPart) -> Result<UploadStatus, String> {
        uuid(&request.document_id)?;
        scope(&request.scope)?;
        Self::name(&request.name)?;
        if request.source_bytes == 0
            || request.source_bytes > SOURCE_LIMIT
            || !Self::valid_hash(&request.source_sha256)
            || request.data_base64.len() > CHUNK_BYTES.div_ceil(3) * 4
        {
            return Err("导入来源或分块大小无效，未保存".into());
        }
        let parts = request.source_bytes.div_ceil(CHUNK_BYTES);
        if request.part >= parts {
            return Err("导入分块编号无效".into());
        }
        let bytes = Zeroizing::new(
            STANDARD
                .decode(&request.data_base64)
                .map_err(|_| "文档分块编码无效")?,
        );
        if bytes.len() != (request.source_bytes - request.part * CHUNK_BYTES).min(CHUNK_BYTES) {
            return Err("文档分块字节数不一致，未保存".into());
        }
        let header = Header {
            schema: 1,
            id: request.document_id.clone(),
            scope: request.scope.clone(),
            name: request.name,
            source_sha256: request.source_sha256,
            source_bytes: request.source_bytes,
            parts,
        };
        let _gate = self.gate.lock().await;
        let _lock = self.lock()?;
        let dir = self.dir(&request.document_id)?;
        if !dir.exists() {
            let mut count = 0;
            for entry in fs::read_dir(&self.root).map_err(|_| "文档目录不可读")? {
                if entry
                    .map_err(|_| "文档条目不可读")?
                    .file_type()
                    .map_err(|_| "文档类型不可确认")?
                    .is_dir()
                {
                    count += 1;
                }
            }
            if count >= MAX_DOCUMENTS {
                return Err("文档数量已达上限，未删除原来源".into());
            }
        }
        crate::control_protocol::private_directory(&dir)
            .await
            .map_err(|_| "分块导入目录无法保护")?;
        let meta_file = dir.join("meta.dpapi");
        if meta_file.exists() {
            let meta = self.metadata(&request.document_id)?;
            if meta.origin_scope != header.scope
                || meta.name != header.name
                || meta.source_sha256 != header.source_sha256
                || meta.source_bytes != header.source_bytes
            {
                return Err("文档编号已用于其他来源，原文件保留".into());
            }
        }
        let header_file = dir.join("upload.dpapi");
        if header_file.exists() {
            if self.upload_header(&request.document_id)? != header {
                return Err("原导入请求身份不一致，未覆盖分块".into());
            }
        } else {
            self.publish(&header_file, &header, 16 * 1024).await?;
        }
        let file = dir.join(format!("upload-{:02}.dpapi", request.part));
        if file.exists() {
            let old: Chunk = self.read(&file, 512 * 1024)?;
            if old.header != header
                || old.part != request.part
                || old.data_base64 != request.data_base64
            {
                return Err("相同分块编号已用于不同内容，原分块保留".into());
            }
        } else {
            self.publish(
                &file,
                &Chunk {
                    header,
                    part: request.part,
                    data_base64: request.data_base64,
                },
                512 * 1024,
            )
            .await?;
        }
        self.upload_status(&request.document_id, &request.scope)
    }
    pub async fn upload_commit(&self, id: &str, owner: &str) -> Result<DocumentMeta, String> {
        let (header, bytes) = {
            let _gate = self.gate.lock().await;
            let _lock = self.lock()?;
            let header = self.upload_header(id)?;
            if header.scope != owner {
                return Err("不能提交其他输入的文档".into());
            }
            let mut bytes = Zeroizing::new(Vec::with_capacity(header.source_bytes));
            for part in 0..header.parts {
                let saved: Chunk = self.read(
                    &self.dir(id)?.join(format!("upload-{part:02}.dpapi")),
                    512 * 1024,
                )?;
                if saved.header != header || saved.part != part {
                    return Err("导入分块身份不一致，未提交".into());
                }
                let decoded = Zeroizing::new(
                    STANDARD
                        .decode(&saved.data_base64)
                        .map_err(|_| "原导入分块编码损坏")?,
                );
                if decoded.len() != (header.source_bytes - part * CHUNK_BYTES).min(CHUNK_BYTES) {
                    return Err("原导入分块字节数不一致".into());
                }
                bytes.extend_from_slice(&decoded);
            }
            if bytes.len() != header.source_bytes || hash(&bytes) != header.source_sha256 {
                return Err("完整文档来源摘要不一致，未提交或解析".into());
            }
            (header, bytes)
        };
        self.stage(id, owner, &header.name, &bytes).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: &str, bytes: &[u8], part: usize) -> UploadPart {
        let from = part * CHUNK_BYTES;
        UploadPart {
            scope: "new".into(),
            document_id: id.into(),
            name: "合成资料.pdf".into(),
            source_sha256: hash(bytes),
            source_bytes: bytes.len(),
            part,
            data_base64: STANDARD.encode(&bytes[from..(from + CHUNK_BYTES).min(bytes.len())]),
        }
    }
    #[tokio::test]
    async fn document_upload_resumes_out_of_order_and_replays_without_overwriting_ciphertext() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("docs");
        let store = DocumentStore::new(root.clone());
        let id = uuid::Uuid::new_v4().to_string();
        let bytes = vec![b'x'; CHUNK_BYTES + 31];
        let last = request(&id, &bytes, 1);
        let partial = store.upload_part(last.clone()).await.unwrap();
        assert_eq!(partial.missing, vec![0]);
        assert_eq!(partial.bytes_received, 31);
        assert!(!partial.complete && !partial.committed);
        assert_eq!(store.metadata_index().unwrap().1, 0);
        assert!(store.upload_commit(&id, "new").await.is_err());
        assert!(store
            .upload_status(&id, &uuid::Uuid::new_v4().to_string())
            .is_err());
        let part_file = root.join(&id).join("upload-01.dpapi");
        let cipher = fs::read(&part_file).unwrap();
        let reopened = DocumentStore::new(root.clone());
        reopened.upload_part(last.clone()).await.unwrap();
        assert_eq!(fs::read(&part_file).unwrap(), cipher);
        let mut conflict = last;
        conflict.data_base64 = STANDARD.encode(vec![b'y'; 31]);
        assert!(reopened.upload_part(conflict).await.is_err());
        assert_eq!(fs::read(&part_file).unwrap(), cipher);
        reopened.upload_part(request(&id, &bytes, 0)).await.unwrap();
        let complete = reopened.upload_status(&id, "new").unwrap();
        assert!(complete.complete && !complete.committed);
        reopened.upload_commit(&id, "new").await.unwrap();
        let source_file = root.join(&id).join("source.dpapi");
        let source_cipher = fs::read(&source_file).unwrap();
        assert!(!source_cipher.windows(32).any(|w| w == &bytes[..32]));
        reopened.upload_commit(&id, "new").await.unwrap();
        assert_eq!(fs::read(&source_file).unwrap(), source_cipher);
        assert!(reopened.upload_status(&id, "new").unwrap().committed);
        assert!(reopened.upload_index("new").unwrap().is_empty());
        let access = reopened.access(&id, "new", false, false).unwrap();
        assert_eq!(&*reopened.source(&access).unwrap(), &bytes);
    }
    #[tokio::test]
    async fn document_upload_never_commits_bad_full_hash_or_invalid_part_identity() {
        let temp = tempfile::tempdir().unwrap();
        let store = DocumentStore::new(temp.path().join("docs"));
        let id = uuid::Uuid::new_v4().to_string();
        let bytes = b"synthetic pdf source";
        let mut input = request(&id, bytes, 0);
        input.source_sha256 = "a".repeat(64);
        store.upload_part(input.clone()).await.unwrap();
        assert!(store.upload_status(&id, "new").unwrap().complete);
        assert!(store.upload_commit(&id, "new").await.is_err());
        assert!(!store.registered(&id).unwrap());
        let mut bad = input.clone();
        bad.part = 1;
        assert!(store.upload_part(bad).await.is_err());
        let mut bad = input;
        bad.name = "../private.pdf".into();
        assert!(store.upload_part(bad).await.is_err());
        assert!(!store.registered(&id).unwrap());
    }
    #[tokio::test]
    async fn document_upload_accepts_full_four_mib_with_every_control_frame_below_one_mib() {
        let temp = tempfile::tempdir().unwrap();
        let store = DocumentStore::new(temp.path().join("docs"));
        let id = uuid::Uuid::new_v4().to_string();
        let bytes = vec![42; SOURCE_LIMIT];
        for part in 0..16 {
            let input = request(&id, &bytes, part);
            assert!(serde_json::to_vec(&input).unwrap().len() < 1024 * 1024 - 4096);
            store.upload_part(input).await.unwrap();
        }
        let status = store.upload_status(&id, "new").unwrap();
        assert_eq!(status.bytes_received, SOURCE_LIMIT);
        store.upload_commit(&id, "new").await.unwrap();
        let access = store.access(&id, "new", false, false).unwrap();
        assert_eq!(store.source(&access).unwrap().len(), SOURCE_LIMIT);
    }
}
