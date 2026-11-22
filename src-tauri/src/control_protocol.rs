//! Authenticated local control transport. Independent of Tauri for isolated tests.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
};

pub const PROTOCOL: u32 = 1;
pub const MAX_REQUEST: usize = 1024 * 1024;
pub const MAX_RESPONSE: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}
impl ControlError {
    pub fn new(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol: u32,
    pub id: String,
    pub token: String,
    pub method: String,
    pub catalog_hash: Option<String>,
    pub command: Option<String>,
    #[serde(default = "object")]
    pub args: Value,
    #[serde(default)]
    pub grants: Vec<String>,
    pub request_id: Option<String>,
    pub job_id: Option<String>,
}
fn object() -> Value {
    json!({})
}
pub type Handler = Arc<
    dyn Fn(Request) -> Pin<Box<dyn Future<Output = Result<Value, ControlError>> + Send>>
        + Send
        + Sync,
>;

#[derive(Serialize, Deserialize)]
pub struct Endpoint {
    pub protocol: u32,
    pub endpoint: String,
    pub token: String,
    pub instance_id: String,
    pub pid: u32,
    pub catalog_hash: String,
}

pub struct Server {
    listener: TcpListener,
    token: String,
    endpoint_path: PathBuf,
    instance_id: String,
    _lock: std::fs::File,
}

impl Server {
    pub async fn bind(directory: &Path, catalog_hash: &str) -> Result<Self, ControlError> {
        std::fs::create_dir_all(directory)
            .map_err(|_| ControlError::new("STATE_IO", "无法创建控制服务目录"))?;
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("server.lock"))
            .map_err(|_| ControlError::new("STATE_IO", "无法打开控制服务锁"))?;
        fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
            ControlError::new("ALREADY_RUNNING", "该目录已有控制服务，不会覆盖其端点")
        })?;
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|_| ControlError::new("BIND_FAILED", "无法监听本机控制端口"))?;
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let instance_id = uuid::Uuid::new_v4().to_string();
        let endpoint = Endpoint {
            protocol: PROTOCOL,
            endpoint: listener.local_addr().unwrap().to_string(),
            token: token.clone(),
            instance_id: instance_id.clone(),
            pid: std::process::id(),
            catalog_hash: catalog_hash.into(),
        };
        let endpoint_path = directory.join("endpoint.json");
        publish_private(&endpoint_path, &serde_json::to_vec(&endpoint).unwrap()).await?;
        Ok(Self {
            listener,
            token,
            endpoint_path,
            instance_id,
            _lock: lock,
        })
    }

    pub async fn serve(self, handler: Handler) -> Result<(), ControlError> {
        let permits = Arc::new(Semaphore::new(16));
        loop {
            let (socket, _) = self
                .listener
                .accept()
                .await
                .map_err(|_| ControlError::new("ACCEPT_FAILED", "本机控制连接失败"))?;
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                drop(socket);
                continue;
            };
            let token = self.token.clone();
            let handler = handler.clone();
            tokio::spawn(async move {
                let _permit = permit;
                connection(socket, &token, handler).await;
            });
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // A new process must never lose its endpoint because an old process exits.
        if let Ok(raw) = std::fs::read(&self.endpoint_path) {
            if serde_json::from_slice::<Endpoint>(&raw)
                .is_ok_and(|value| value.instance_id == self.instance_id)
            {
                let _ = std::fs::remove_file(&self.endpoint_path);
            }
        }
    }
}

async fn connection(socket: TcpStream, expected: &str, handler: Handler) {
    let (read, mut write) = socket.into_split();
    let mut bytes = Vec::new();
    let read_result = tokio::time::timeout(
        Duration::from_secs(5),
        BufReader::new(read)
            .take((MAX_REQUEST + 1) as u64)
            .read_until(b'\n', &mut bytes),
    )
    .await;
    let response = match read_result {
        Ok(Ok(_)) if bytes.len() <= MAX_REQUEST && bytes.last() == Some(&b'\n') => {
            match serde_json::from_slice::<Request>(&bytes) {
                Ok(request) if constant_eq(request.token.as_bytes(), expected.as_bytes()) => {
                    let id = request.id.clone();
                    if request.protocol != PROTOCOL {
                        envelope(
                            &id,
                            Err(ControlError::new("PROTOCOL_MISMATCH", "控制协议不兼容")),
                        )
                    } else if id.is_empty() || id.len() > 128 {
                        envelope("", Err(ControlError::new("BAD_REQUEST", "请求编号无效")))
                    } else {
                        envelope(&id, handler(request).await)
                    }
                }
                Ok(_) => envelope(
                    "",
                    Err(ControlError::new("UNAUTHORIZED", "本机控制认证失败")),
                ),
                Err(_) => envelope("", Err(ControlError::new("BAD_REQUEST", "请求格式无效"))),
            }
        }
        _ => envelope(
            "",
            Err(ControlError::new(
                "REQUEST_LIMIT",
                "请求过大、不完整或读取超时",
            )),
        ),
    };
    let mut encoded = serde_json::to_vec(&response).unwrap();
    if encoded.len() > MAX_RESPONSE {
        encoded = serde_json::to_vec(&envelope(
            response["id"].as_str().unwrap_or(""),
            Err(ControlError::new(
                "RESULT_LIMIT",
                "响应过大，请通过任务的文件结果入口读取",
            )),
        ))
        .unwrap();
    }
    encoded.push(b'\n');
    let _ = tokio::time::timeout(Duration::from_secs(5), write.write_all(&encoded)).await;
}
fn envelope(id: &str, result: Result<Value, ControlError>) -> Value {
    match result {
        Ok(result) => json!({"protocol":PROTOCOL,"id":id,"ok":true,"result":result}),
        Err(error) => json!({"protocol":PROTOCOL,"id":id,"ok":false,"error":error}),
    }
}
fn constant_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[cfg(windows)]
fn protect_for_current_user(path: &Path, directory: bool) -> Result<(), ControlError> {
    use std::{
        os::windows::{
            ffi::OsStrExt,
            io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        ptr::{null, null_mut},
    };
    use windows_sys::Win32::{
        Foundation::{LocalFree, ERROR_SUCCESS},
        Security::{
            Authorization::{
                SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, NO_MULTIPLE_TRUSTEE,
                SET_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
            },
            GetTokenInformation, TokenUser, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
            OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER,
        },
        Storage::FileSystem::FILE_ALL_ACCESS,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    };

    let user_error = || ControlError::new("STATE_ACL", "无法识别本机用户");
    let acl_error = || ControlError::new("STATE_ACL", "无法保护私有文件或目录");
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(user_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let token_handle = token.as_raw_handle();
    let mut bytes = 0;
    unsafe { GetTokenInformation(token_handle, TokenUser, null_mut(), 0, &mut bytes) };
    if (bytes as usize) < std::mem::size_of::<TOKEN_USER>() {
        return Err(user_error());
    }
    // Keep the buffer aligned for TOKEN_USER and alive while the SID is in use.
    let mut buffer = vec![0usize; (bytes as usize).div_ceil(std::mem::size_of::<usize>())];
    let user = buffer.as_mut_ptr().cast::<TOKEN_USER>();
    let read =
        unsafe { GetTokenInformation(token_handle, TokenUser, user.cast(), bytes, &mut bytes) };
    if read == 0 {
        return Err(user_error());
    }

    let wide: Vec<_> = std::fs::canonicalize(path)
        .map_err(|_| acl_error())?
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: if directory {
            OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
        } else {
            0
        },
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: unsafe { (*user).User.Sid.cast() },
        },
    };

    let mut acl = null_mut();
    if unsafe { SetEntriesInAclW(1, &entry, null(), &mut acl) } != ERROR_SUCCESS {
        return Err(acl_error());
    }
    let status = unsafe {
        let status = SetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null_mut(),
        );
        LocalFree(acl.cast());
        status
    };
    if status != ERROR_SUCCESS {
        return Err(acl_error());
    }
    Ok(())
}

pub(crate) async fn private_directory(path: &Path) -> Result<(), ControlError> {
    std::fs::create_dir_all(path).map_err(|_| ControlError::new("STATE_IO", "无法创建私有目录"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| ControlError::new("STATE_ACL", "无法保护目录"))?;
    }
    #[cfg(windows)]
    protect_for_current_user(path, true)?;
    Ok(())
}

fn private_write_error(
    message: &str,
    stage: &'static str,
    failure: std::io::Error,
) -> ControlError {
    let mut error = ControlError::new("STATE_IO", message);
    error.details = Some(json!({ "stage": stage, "os_code": failure.raw_os_error() }));
    error
}

pub(crate) async fn publish_private(path: &Path, content: &[u8]) -> Result<(), ControlError> {
    use std::io::Write;
    // tempfile's Windows persist uses MoveFileExW directly. Rust filesystem
    // operations accept a long ordinary path, but that final Win32 call needs
    // the extended-length path produced by canonicalizing the existing parent.
    // Resolve before staging so ACL protection and publication use one directory.
    #[cfg(windows)]
    let destination = {
        let parent = path
            .parent()
            .filter(|value| !value.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let parent = std::fs::canonicalize(parent)
            .map_err(|failure| private_write_error("无法定位私有文件目录", "parent", failure))?;
        let name = path
            .file_name()
            .ok_or_else(|| ControlError::new("STATE_IO", "私有文件名称无效"))?;
        parent.join(name)
    };
    #[cfg(windows)]
    let path = destination.as_path();
    let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())
        .map_err(|failure| private_write_error("无法准备控制端点", "staging", failure))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        staged
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| ControlError::new("STATE_ACL", "无法保护控制端点权限"))?;
    }
    #[cfg(windows)]
    protect_for_current_user(staged.path(), false)?;
    // Apply owner-only permissions before writing protected local contents.
    staged
        .write_all(content)
        .and_then(|_| staged.as_file().sync_all())
        .map_err(|failure| private_write_error("控制文件写入失败", "write", failure))?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let staged = staged.into_temp_path();
        let source: Vec<_> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // Same-directory replacement, without a copy/delete fallback.
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                target.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(private_write_error(
                "控制文件发布失败",
                "replace",
                std::io::Error::last_os_error(),
            ));
        }
    }
    #[cfg(not(windows))]
    staged
        .persist(path)
        .map_err(|failure| private_write_error("控制文件发布失败", "replace", failure.error))?;
    #[cfg(unix)]
    std::fs::File::open(path.parent().unwrap())
        .and_then(|directory| directory.sync_all())
        .map_err(|failure| {
            private_write_error("控制文件目录同步失败", "directory_sync", failure)
        })?;
    Ok(())
}

#[cfg(all(test, windows))]
mod private_path_tests {
    use super::*;
    use std::os::windows::ffi::OsStrExt;

    fn long_directory(root: &Path) -> PathBuf {
        let mut directory = root.join("中文🙂私有保存");
        while directory.as_os_str().encode_wide().count() < 300 {
            directory.push("synthetic-long-directory");
        }
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    async fn assert_owner_only(path: &Path, audit: &Path) {
        let output = tokio::process::Command::new("whoami.exe")
            .args(["/user", "/fo", "csv", "/nh"])
            .creation_flags(0x08000000)
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        let user = String::from_utf8_lossy(&output.stdout);
        let sid = user
            .split('"')
            .find(|part| part.starts_with("S-1-"))
            .unwrap();
        let output = tokio::process::Command::new("icacls.exe")
            .arg(std::fs::canonicalize(path).unwrap())
            .arg("/save")
            .arg(audit)
            .arg("/Q")
            .creation_flags(0x08000000)
            .output()
            .await
            .unwrap();
        assert!(output.status.success(), "Could not inspect synthetic ACL");
        let bytes = std::fs::read(audit).unwrap();
        let saved = if bytes.starts_with(&[0xff, 0xfe])
            || bytes.windows(4).any(|part| part == [b'D', 0, b':', 0])
        {
            let start = if bytes.starts_with(&[0xff, 0xfe]) {
                2
            } else {
                0
            };
            let words: Vec<_> = bytes[start..]
                .chunks_exact(2)
                .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                .collect();
            String::from_utf16(&words).unwrap()
        } else {
            String::from_utf8(bytes).unwrap()
        };
        let dacl = saved.lines().find(|line| line.starts_with("D:")).unwrap();
        assert!(dacl.contains("P"), "Inheritance must remain disabled");
        assert_eq!(
            dacl.matches("(A;").count(),
            1,
            "Only the current owner is granted access"
        );
        assert!(dacl.contains(sid));
        assert!(dacl.contains(";FA;"), "The owner must retain full access");
        assert!(!dacl.contains("(D;"));
    }

    #[tokio::test]
    async fn private_publish_windows_long_path_replaces_atomically_with_owner_only_permissions() {
        let root = tempfile::tempdir().unwrap();
        let parent = long_directory(root.path());
        let destination = parent.join("合成状态🙂.dpapi");
        assert!(destination.as_os_str().encode_wide().count() > 260);
        publish_private(&destination, b"synthetic-v1")
            .await
            .unwrap();
        publish_private(&destination, b"synthetic-v2")
            .await
            .unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"synthetic-v2");
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 1);
        assert_owner_only(&destination, &root.path().join("file-acl.txt")).await;

        let collision = parent.join("existing-directory");
        std::fs::create_dir(&collision).unwrap();
        std::fs::write(collision.join("sentinel.txt"), b"preserve").unwrap();
        assert!(publish_private(&collision, b"must-not-replace")
            .await
            .is_err());
        assert_eq!(
            std::fs::read(collision.join("sentinel.txt")).unwrap(),
            b"preserve"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"synthetic-v2");
        assert_eq!(std::fs::read_dir(&parent).unwrap().count(), 2);
    }

    #[tokio::test]
    async fn private_directory_windows_long_path_keeps_owner_only_permissions() {
        let root = tempfile::tempdir().unwrap();
        let directory = long_directory(root.path());
        private_directory(&directory).await.unwrap();
        assert_owner_only(&directory, &root.path().join("directory-acl.txt")).await;
    }
}
