use std::{iter::once, mem::size_of, os::windows::ffi::OsStrExt, path::Path, ptr::null};
use windows_sys::Win32::{
    Foundation::{
        GetLastError, ERROR_ACCESS_DENIED, ERROR_CANCELLED, ERROR_FILE_NOT_FOUND,
        ERROR_NO_ASSOCIATION, ERROR_PATH_NOT_FOUND, ERROR_SHARING_VIOLATION, WIN32_ERROR,
    },
    System::Com::{
        CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
    },
    UI::{
        Shell::{
            AssocQueryStringW, ShellExecuteExW, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_PROGID,
            SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW,
        },
        WindowsAndMessaging::SW_SHOWNORMAL,
    },
};

pub(super) fn open(path: &Path) -> Result<(), String> {
    let Some(extension) = path.extension() else {
        return Err(open_error(ERROR_NO_ASSOCIATION).to_owned());
    };
    let extension: Vec<u16> = once(u16::from(b'.'))
        .chain(extension.encode_wide())
        .chain([0])
        .collect();
    let windows_path: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut request = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_FLAG_NO_UI | SEE_MASK_NOASYNC,
        // The default file action may be named Preview rather than Open.
        lpVerb: null(),
        lpFile: windows_path.as_ptr(),
        nShow: SW_SHOWNORMAL,
        ..Default::default()
    };

    // This function runs off the UI thread. Both S_OK and S_FALSE require balanced cleanup.
    let initialized = unsafe {
        CoInitializeEx(
            null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        )
    };
    if initialized < 0 {
        return Err(open_error(0).to_owned());
    }
    let error = unsafe {
        // ShellExecuteEx can accept the generic Open With dialog for an unknown type.
        let mut length = 0;
        let association = AssocQueryStringW(
            ASSOCF_INIT_IGNOREUNKNOWN,
            ASSOCSTR_PROGID,
            extension.as_ptr(),
            null(),
            std::ptr::null_mut(),
            &mut length,
        );
        let no_association = (0x8007_0000 | ERROR_NO_ASSOCIATION) as i32;
        let error = match association {
            code if code == no_association => Some(ERROR_NO_ASSOCIATION),
            code if code >= 0 && length > 1 => {
                let opened = ShellExecuteExW(&mut request);
                (opened == 0).then(|| GetLastError())
            }
            _ => Some(0),
        };
        CoUninitialize();
        error
    };
    error.map_or(Ok(()), |code| Err(open_error(code).to_owned()))
}

fn open_error(code: WIN32_ERROR) -> &'static str {
    match code {
        ERROR_NO_ASSOCIATION => {
            "ARTIFACT_OPEN_NO_ASSOCIATION: Windows 没有可打开此文件类型的关联应用，可以导出副本。"
        }
        ERROR_FILE_NOT_FOUND | ERROR_PATH_NOT_FOUND => {
            "ARTIFACT_OPEN_FILE_MISSING: 交给 Windows 打开时找不到原文件，请核对原文件。"
        }
        ERROR_ACCESS_DENIED | ERROR_SHARING_VIOLATION => {
            "ARTIFACT_OPEN_ACCESS: Windows 无法访问原文件，请检查文件权限或占用情况。"
        }
        ERROR_CANCELLED => "ARTIFACT_OPEN_CANCELLED: 系统打开操作已取消。",
        _ => "ARTIFACT_OPEN_FAILED: Windows 未接受打开请求，可以导出副本。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_open_returns_the_real_windows_error_for_a_missing_file() {
        let directory = tempfile::tempdir().unwrap();
        let error = open(&directory.path().join("不存在的结果.txt")).unwrap_err();
        assert!(error.starts_with("ARTIFACT_OPEN_FILE_MISSING:"), "{error}");
    }

    #[test]
    fn artifact_open_distinguishes_unassociated_files_from_permissions_or_cancellation() {
        let directory = tempfile::tempdir().unwrap();
        let artifact = directory.path().join("隔离结果.homebase_no_handler_4866c1");
        std::fs::write(&artifact, b"synthetic output").unwrap();
        let error = open(&artifact).unwrap_err();
        assert!(
            error.starts_with("ARTIFACT_OPEN_NO_ASSOCIATION:"),
            "{error}"
        );
        assert!(open_error(ERROR_ACCESS_DENIED).starts_with("ARTIFACT_OPEN_ACCESS:"));
        assert!(open_error(ERROR_CANCELLED).starts_with("ARTIFACT_OPEN_CANCELLED:"));
    }
}
