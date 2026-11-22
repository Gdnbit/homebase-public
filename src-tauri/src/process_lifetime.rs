//! Bind a Windows child to the lifetime of its owning host.
use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
};

pub(crate) struct ProcessLifetime(OwnedHandle);

impl ProcessLifetime {
    pub(crate) fn attach(child: &tokio::process::Child) -> io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        let process = child
            .raw_handle()
            .ok_or_else(|| io::Error::other("child process handle unavailable"))?;
        // Null security attributes make the job handle non-inheritable.
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self(unsafe { OwnedHandle::from_raw_handle(raw) });
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job.0.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const _,
                std::mem::size_of_val(&limits) as u32,
            )
        };
        if configured == 0
            || unsafe { AssignProcessToJobObject(job.0.as_raw_handle(), process) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }
}
