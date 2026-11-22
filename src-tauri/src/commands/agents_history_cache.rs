use super::HistoryEntry;
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    mem::size_of,
    path::{Path, PathBuf},
    sync::Mutex,
};

const MAX_CACHE_BYTES: usize = 4 * 1024 * 1024;
const MAX_CACHED_FILES: usize = 4096;
pub(super) const HISTORY_READ_FAILED: &str = "QUERY_FAILED: 历史会话文件不可读取";
const HISTORY_CHANGED: &str = "QUERY_FAILED: 历史会话文件在读取期间发生变化，请重试";

#[derive(Default)]
pub(super) struct SummaryCache {
    files: HashMap<PathBuf, CachedFile>,
    bytes: usize,
}

struct CachedFile {
    stamp: FileStamp,
    entries: Vec<HistoryEntry>,
    bytes: usize,
}

// A provider lock makes concurrent history lists share the same successful parse.
// The caller supplies the current directory listing so removed files drop out.
pub(super) fn read_summaries(
    cache: &Mutex<SummaryCache>,
    paths: &[PathBuf],
    parse: impl Fn(&Path, &File) -> Result<Vec<HistoryEntry>, String>,
) -> Result<Vec<HistoryEntry>, String> {
    let mut cache = cache
        .lock()
        .map_err(|_| "QUERY_FAILED: 历史摘要缓存不可用")?;
    let current: HashSet<&Path> = paths.iter().map(PathBuf::as_path).collect();
    let mut retained_bytes = 0;
    cache.files.retain(|path, entry| {
        let retained = current.contains(path.as_path());
        if retained {
            retained_bytes += entry.bytes;
        }
        retained
    });
    cache.bytes = retained_bytes;

    let mut summaries = Vec::new();
    for path in paths {
        cache.append(path, &parse, &mut summaries)?;
    }
    Ok(summaries)
}

impl SummaryCache {
    fn append(
        &mut self,
        path: &Path,
        parse: &impl Fn(&Path, &File) -> Result<Vec<HistoryEntry>, String>,
        summaries: &mut Vec<HistoryEntry>,
    ) -> Result<(), String> {
        let file = open_history(path)?;
        let stamp = file_stamp(&file);
        if let Some(cached) = self.files.get(path) {
            if stamp.as_ref() == Some(&cached.stamp) {
                summaries.extend(cached.entries.iter().cloned());
                return Ok(());
            }
        }
        if let Some(previous) = self.files.remove(path) {
            self.bytes -= previous.bytes;
        }

        let cacheable = stamp.as_ref().is_some_and(FileStamp::is_stable);
        let entries = parse(path, &file)?;
        if let Some(stamp) = stamp {
            // Check the named file again so an atomic replacement cannot publish
            // the old handle's summary under the replacement's path.
            if file_stamp(&file).as_ref() != Some(&stamp)
                || File::open(path)
                    .ok()
                    .and_then(|current| file_stamp(&current))
                    .as_ref()
                    != Some(&stamp)
            {
                return Err(HISTORY_CHANGED.into());
            }
            if cacheable && self.files.len() < MAX_CACHED_FILES {
                let bytes = summary_bytes(path, &entries);
                if bytes <= MAX_CACHE_BYTES - self.bytes {
                    summaries.extend(entries.iter().cloned());
                    self.files.insert(
                        path.to_owned(),
                        CachedFile {
                            stamp,
                            entries,
                            bytes,
                        },
                    );
                    self.bytes += bytes;
                    return Ok(());
                }
            }
        }
        summaries.extend(entries);
        Ok(())
    }
}

pub(super) fn open_matching_history(
    cache: &Mutex<SummaryCache>,
    path: &Path,
    session_id: &str,
) -> Result<Option<File>, String> {
    let file = open_history(path)?;
    let stamp = file_stamp(&file);
    let cache = cache
        .lock()
        .map_err(|_| "QUERY_FAILED: 历史摘要缓存不可用")?;
    if cache.files.get(path).is_some_and(|cached| {
        stamp.as_ref() == Some(&cached.stamp)
            && cached
                .entries
                .iter()
                .all(|entry| entry.session_id != session_id)
    }) {
        return Ok(None);
    }
    Ok(Some(file))
}

fn summary_bytes(path: &Path, entries: &[HistoryEntry]) -> usize {
    size_of::<(PathBuf, CachedFile)>()
        + path.as_os_str().len()
        + std::mem::size_of_val(entries)
        + entries
            .iter()
            .map(|entry| {
                entry.agent_id.capacity()
                    + entry.session_id.capacity()
                    + entry.cwd.as_ref().map_or(0, String::capacity)
                    + entry.first_user_prompt.as_ref().map_or(0, String::capacity)
                    + entry.started_at.capacity()
                    + entry.last_active_at.capacity()
                    + entry.file_path.capacity()
            })
            .sum::<usize>()
}

pub(super) fn open_history(path: &Path) -> Result<File, String> {
    File::open(path).map_err(|_| HISTORY_READ_FAILED.into())
}

#[cfg(windows)]
#[derive(PartialEq, Eq)]
struct FileStamp {
    volume: u64,
    identity: [u8; 16],
    written: i64,
    changed: i64,
    length: u64,
}

#[cfg(windows)]
const FILE_TIME_SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(windows)]
impl FileStamp {
    fn is_stable(&self) -> bool {
        use std::time::{SystemTime, UNIX_EPOCH};
        const UNIX_EPOCH_FILE_TIME: u128 = 116_444_736_000_000_000;
        // Rapid Windows writes can share a timestamp. Only cache a summary if
        // the timestamp was already settled before reading the file.
        let latest = self.changed.max(self.written);
        latest > 0
            && SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .is_ok_and(|elapsed| {
                    let now = UNIX_EPOCH_FILE_TIME + elapsed.as_nanos() / 100;
                    now >= latest as u128 + FILE_TIME_SETTLE.as_nanos() / 100
                })
    }
}

#[cfg(not(windows))]
impl FileStamp {
    fn is_stable(&self) -> bool {
        true
    }
}

#[cfg(windows)]
fn file_stamp(file: &File) -> Option<FileStamp> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileBasicInfo, FileIdInfo, GetFileInformationByHandleEx, FILE_BASIC_INFO, FILE_ID_INFO,
    };
    let mut basic = FILE_BASIC_INFO::default();
    let mut id = FILE_ID_INFO::default();
    let handle = file.as_raw_handle();
    // The borrowed File keeps the handle alive. Both buffers match the requested classes.
    let valid = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileBasicInfo,
            (&mut basic as *mut FILE_BASIC_INFO).cast(),
            size_of::<FILE_BASIC_INFO>() as u32,
        ) != 0
            && GetFileInformationByHandleEx(
                handle,
                FileIdInfo,
                (&mut id as *mut FILE_ID_INFO).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            ) != 0
    };
    if !valid || basic.ChangeTime == 0 || id.FileId.Identifier == [0; 16] {
        return None;
    }
    Some(FileStamp {
        volume: id.VolumeSerialNumber,
        identity: id.FileId.Identifier,
        written: basic.LastWriteTime,
        changed: basic.ChangeTime,
        length: file.metadata().ok()?.len(),
    })
}

#[cfg(unix)]
#[derive(PartialEq, Eq)]
struct FileStamp {
    device: u64,
    inode: u64,
    written: (i64, i64),
    changed: (i64, i64),
    length: u64,
}

#[cfg(unix)]
fn file_stamp(file: &File) -> Option<FileStamp> {
    use std::os::unix::fs::MetadataExt;
    let info = file.metadata().ok()?;
    Some(FileStamp {
        device: info.dev(),
        inode: info.ino(),
        written: (info.mtime(), info.mtime_nsec()),
        changed: (info.ctime(), info.ctime_nsec()),
        length: info.len(),
    })
}

#[cfg(not(any(windows, unix)))]
#[derive(PartialEq, Eq)]
struct FileStamp;

#[cfg(not(any(windows, unix)))]
fn file_stamp(_: &File) -> Option<FileStamp> {
    None
}

#[cfg(all(test, any(windows, unix)))]
mod tests {
    use super::*;
    use std::{
        fs::{self, FileTimes, OpenOptions},
        io::{Read, Write},
        sync::atomic::{AtomicUsize, Ordering},
    };

    fn read_fixture(file: &File) -> Result<Vec<HistoryEntry>, String> {
        let mut content = String::new();
        let mut file = file;
        file.read_to_string(&mut content).unwrap();
        let (id, preview) = content.split_once(':').ok_or("QUERY_INVALID: fixture")?;
        Ok(vec![HistoryEntry {
            agent_id: "gemini-cli".into(),
            session_id: id.into(),
            cwd: None,
            first_user_prompt: Some(preview.into()),
            started_at: String::new(),
            last_active_at: String::new(),
            message_count: 1,
            file_path: String::new(),
        }])
    }

    #[test]
    fn unchanged_files_share_one_content_parse_and_skip_unrelated_details() {
        let dir = tempfile::tempdir().unwrap();
        let paths = [dir.path().join("a"), dir.path().join("b")];
        fs::write(&paths[0], "one:初始").unwrap();
        fs::write(&paths[1], "two:完整").unwrap();
        #[cfg(windows)]
        std::thread::sleep(FILE_TIME_SETTLE);
        let cache = Mutex::default();
        let parses = AtomicUsize::new(0);
        let scan = || {
            read_summaries(&cache, &paths, |_, file| {
                parses.fetch_add(1, Ordering::Relaxed);
                read_fixture(file)
            })
            .unwrap()
        };
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..6).map(|_| scope.spawn(scan)).collect();
            for worker in workers {
                assert_eq!(worker.join().unwrap().len(), 2);
            }
        });
        assert_eq!(parses.load(Ordering::Relaxed), 2);
        assert!(open_matching_history(&cache, &paths[1], "one")
            .unwrap()
            .is_none());
        assert!(open_matching_history(&cache, &paths[0], "one")
            .unwrap()
            .is_some());

        OpenOptions::new()
            .append(true)
            .open(&paths[1])
            .unwrap()
            .write_all("的新消息".as_bytes())
            .unwrap();
        assert!(open_matching_history(&cache, &paths[1], "one")
            .unwrap()
            .is_some());
        assert_eq!(scan()[1].first_user_prompt.as_deref(), Some("完整的新消息"));
        assert_eq!(parses.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn same_length_and_write_time_replacement_invalidates_and_damage_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("current");
        let paths = [path.clone()];
        let replacement = dir.path().join("replacement");
        let cache = Mutex::default();
        let scan = || read_summaries(&cache, &paths, |_, file| read_fixture(file));
        fs::write(&path, "one:old").unwrap();
        #[cfg(windows)]
        std::thread::sleep(FILE_TIME_SETTLE);
        assert_eq!(scan().unwrap()[0].session_id, "one");
        let written = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&replacement, "two:new").unwrap();
        File::options()
            .write(true)
            .open(&replacement)
            .unwrap()
            .set_times(FileTimes::new().set_modified(written))
            .unwrap();
        fs::rename(&replacement, &path).unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), written);
        assert_eq!(scan().unwrap()[0].session_id, "two");
        #[cfg(windows)]
        assert!(cache.lock().unwrap().files.is_empty());

        fs::write(&path, "invalid").unwrap();
        assert!(scan().unwrap_err().starts_with("QUERY_INVALID:"));
        fs::write(&path, "one:fixed").unwrap();
        assert_eq!(
            scan().unwrap()[0].first_user_prompt.as_deref(),
            Some("fixed")
        );
        fs::remove_file(&path).unwrap();
        assert!(read_summaries(&cache, &[], |_, _| unreachable!())
            .unwrap()
            .is_empty());
        assert!(cache.lock().unwrap().files.is_empty());
    }

    #[test]
    fn replacement_during_parse_is_rejected_and_large_summary_is_not_retained() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("current");
        let paths = [path.clone()];
        let replacement = dir.path().join("replacement");
        let cache = Mutex::default();
        fs::write(&path, "one:old").unwrap();
        fs::write(&replacement, "two:new").unwrap();
        let failed = read_summaries(&cache, &paths, |_, file| {
            let old = read_fixture(file)?;
            fs::rename(&replacement, &path).unwrap();
            Ok(old)
        });
        assert!(failed.unwrap_err().contains("读取期间发生变化"));
        assert!(cache.lock().unwrap().files.is_empty());

        let summary = read_summaries(&cache, &paths, |_, file| {
            let mut result = read_fixture(file)?;
            result[0].first_user_prompt = Some("x".repeat(MAX_CACHE_BYTES));
            Ok(result)
        })
        .unwrap();
        assert_eq!(summary[0].session_id, "two");
        assert_eq!(
            summary[0].first_user_prompt.as_ref().unwrap().len(),
            MAX_CACHE_BYTES
        );
        assert!(cache.lock().unwrap().files.is_empty());
    }
}
