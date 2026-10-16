// SPDX-License-Identifier: AGPL-3.0-only
//! A Windows job object that kills every engine process when the app exits,
//! including after a crash, so no model is left holding memory.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_LIMIT_PRIORITY_CLASS,
};
use windows::Win32::System::Threading::{OpenProcess, BELOW_NORMAL_PRIORITY_CLASS, PROCESS_SET_QUOTA, PROCESS_TERMINATE};

pub struct Job(HANDLE);

// SAFETY: a job handle can be used from any thread.
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Job {
    pub fn kill_on_close() -> windows::core::Result<Job> {
        // SAFETY: we own the handle and pass a correctly sized struct.
        unsafe {
            let handle = CreateJobObjectW(None, PCWSTR::null())?;
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
            Ok(Job(handle))
        }
    }

    /// For the assistant's commands: they end with the app too, can't take
    /// more than `memory` bytes or `processes` processes between them, and
    /// run at below-normal priority, so a runaway one can't freeze the PC.
    pub fn limited(memory: u64, processes: u32) -> windows::core::Result<Job> {
        // SAFETY: as in kill_on_close; the handle is closed if setting fails.
        unsafe {
            let job = Job(CreateJobObjectW(None, PCWSTR::null())?);
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags =
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_JOB_MEMORY | JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_PRIORITY_CLASS;
            info.BasicLimitInformation.ActiveProcessLimit = processes;
            info.BasicLimitInformation.PriorityClass = BELOW_NORMAL_PRIORITY_CLASS.0;
            info.JobMemoryLimit = memory as usize;
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )?;
            Ok(job)
        }
    }

    pub fn assign(&self, pid: u32) -> windows::core::Result<()> {
        // SAFETY: the process handle is closed before returning.
        unsafe {
            let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, false, pid)?;
            let result = AssignProcessToJobObject(self.0, process);
            let _ = CloseHandle(process);
            result
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: closing our own handle; this also ends assigned processes.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
