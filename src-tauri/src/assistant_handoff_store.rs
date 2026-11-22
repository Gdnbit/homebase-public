//! Completed empty outboxes retain their revision without consuming active quota.
//! No source material, pending request or recording can enter this directory.
use super::*;

const COMPLETED: &str = "completed-handoffs";

impl DesktopStore {
    pub(super) fn completed_handoff(entry: &DesktopEntry) -> bool {
        entry.scope.starts_with("handoff:")
            && entry.revision > 0
            && entry.body == DesktopBody::default()
            && entry.pending.is_none()
            && entry.native_pending.is_none()
            && entry.voices.is_empty()
    }

    pub(super) fn completed_handoff_path(&self, scope: &str) -> Result<PathBuf, String> {
        if !scope.starts_with("handoff:") {
            return Err("非交接暂存不能使用已完成范围".into());
        }
        let active = self.path(scope)?;
        let filename = active.file_name().ok_or("交接暂存编号无效")?;
        Ok(self.directory.join(COMPLETED).join(filename))
    }

    // Read paths never create directories or follow a completed-area junction.
    fn completed_directory_exists(&self) -> Result<bool, String> {
        match fs::symlink_metadata(self.directory.join(COMPLETED)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err("已完成交接目录不可读，未回退为空".into()),
            Ok(metadata) => {
                #[cfg(windows)]
                let reparse = {
                    use std::os::windows::fs::MetadataExt;
                    metadata.file_attributes() & 0x400 != 0
                };
                #[cfg(not(windows))]
                let reparse = metadata.file_type().is_symlink();
                if !metadata.is_dir() || reparse {
                    Err("已完成交接目录类型无效，未跟随链接或覆盖原文件".into())
                } else {
                    Ok(true)
                }
            }
        }
    }

    pub(super) fn read_completed_handoff(
        &self,
        scope: &str,
    ) -> Result<Option<DesktopEntry>, String> {
        if !scope.starts_with("handoff:") || !self.completed_directory_exists()? {
            return Ok(None);
        }
        let path = self.completed_handoff_path(scope)?;
        match fs::metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err("已完成交接版本不可读，未重置草稿版本".into()),
            Ok(_) => {
                let entry = self.read_file(&path)?;
                if !Self::completed_handoff(&entry) {
                    return Err("已完成交接含有未完成内容，未作为空草稿读取".into());
                }
                Ok(Some(entry))
            }
        }
    }

    // Called only while save holds both the store gate and the workspace lock.
    // A failed move leaves the confirmed empty file at its original location.
    pub(super) fn finish_completed_handoff(&self, entry: &DesktopEntry) -> Result<(), String> {
        if !Self::completed_handoff(entry) {
            return Ok(());
        }
        let active = self.path(&entry.scope)?;
        match fs::metadata(&active) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err("交接副本已清空，但原文件状态未确认；目标草稿不回滚".into()),
            Ok(_) => {}
        }
        if !self.completed_directory_exists()? {
            fs::create_dir(self.directory.join(COMPLETED))
                .map_err(|_| "交接副本已清空，但已完成目录创建失败；原文件保留")?;
            self.completed_directory_exists()?;
        }
        if let Some(previous) = self.read_completed_handoff(&entry.scope)? {
            if previous.revision >= entry.revision {
                return Err("已完成交接版本不早于原文件，未猜测或覆盖".into());
            }
        }
        fs::rename(&active, self.completed_handoff_path(&entry.scope)?)
            .map_err(|_| "交接副本已清空，但移动未确认；原文件与已保存目标保留".to_owned())
    }
}
