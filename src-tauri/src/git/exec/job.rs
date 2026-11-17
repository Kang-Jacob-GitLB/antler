//! Windows Job Object — git 프로세스 트리를 한 번에, 프로세스 생성 없이 종료한다.
//!
//! 프로세스마다 Job 하나를 만들고 `KILL_ON_JOB_CLOSE` 를 건다. 종료는 동기 `TerminateJobObject` 이고(프로세스를 만들지 않아 런타임을 막지 않는다),
//! Job 핸들이 닫히면(핸들 소유자의 Drop·앱 비정상 종료 포함) 커널이 트리를 죽인다. `taskkill` 은 CreateProcess 가 느리고, 앱이 끝나는 순간
//! 그 스레드가 죽으면 일시 정지된 채 고아로 남았다(docs/tauri.md "실행기").

use std::io;
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use tokio::process::Child;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

// windows 크레이트에 없는 ntdll 함수. 문서화되지는 않았지만 Windows 2000 부터 있고 Sysinternals(pssuspend)·디버거가 쓴다
#[link(name = "ntdll", kind = "raw-dylib")]
extern "system" {
    /// 프로세스의 모든 스레드를 재개한다. 성공은 `NTSTATUS >= 0`
    fn NtResumeProcess(process: HANDLE) -> i32;
}

fn io_err(e: windows::core::Error) -> io::Error {
    io::Error::from(e)
}

/// 프로세스 하나의 트리를 담는 Job. 핸들을 닫으면(Drop) 아직 `KILL_ON_JOB_CLOSE` 가 걸려 있는 한 트리가 죽는다
pub(crate) struct Job(OwnedHandle);

impl Job {
    /// `KILL_ON_JOB_CLOSE` 가 걸린 새 Job
    pub(crate) fn new() -> io::Result<Job> {
        let h = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map_err(io_err)?;
        // CreateJobObjectW 가 돌려준 새 핸들이라 우리가 소유한다
        let job = Job(unsafe { OwnedHandle::from_raw_handle(h.0) });
        job.set_flags(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)?;
        Ok(job)
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.0.as_raw_handle())
    }

    fn set_flags(&self, flags: JOB_OBJECT_LIMIT) -> io::Result<()> {
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = flags;
        unsafe {
            SetInformationJobObject(
                self.handle(),
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        }
        .map_err(io_err)
    }

    /// 프로세스(와 이후 그 자손)를 Job 에 넣는다. 자식을 빌려 받으므로 핸들은 호출이 끝날 때까지 살아 있다(이미 수거됐으면 오류)
    pub(crate) fn assign(&self, child: &Child) -> io::Result<()> {
        let process = child_handle(child)?;
        unsafe { AssignProcessToJobObject(self.handle(), process) }.map_err(io_err)
    }

    /// Job 안의 모든 프로세스를 즉시 종료한다(동기, 프로세스 생성 없음)
    pub(crate) fn terminate(&self) {
        // 이미 비어 있으면 실패해도 무해하다
        let _ = unsafe { TerminateJobObject(self.handle(), 1) };
    }

    /// 핸들을 닫아도 트리를 죽이지 않게 한다(루트가 스스로 끝난 뒤 남은 자손 — 예: git 의 분리된 `gc --auto` — 을 건드리지 않는다)
    pub(super) fn disarm(&self) {
        let _ = self.set_flags(JOB_OBJECT_LIMIT(0));
    }
}

/// 자식이 쥔 프로세스 핸들. 반환값은 `child` 가 살아 있는 동안만 유효하다(호출부가 `child` 를 빌려 쓰는 동안만 쓴다)
fn child_handle(child: &Child) -> io::Result<HANDLE> {
    child.raw_handle().map(HANDLE).ok_or_else(|| io::Error::other("자식 프로세스의 핸들을 얻지 못했다"))
}

/// `CREATE_SUSPENDED` 로 만든 프로세스를 재개한다. 자식을 빌려 받으므로 핸들은 호출이 끝날 때까지 살아 있다.
/// 주 스레드 핸들이 없어(std 가 닫는다) 프로세스 단위로 재개한다. Toolhelp 스레드 스냅샷으로 스레드를 찾는 방법은 문서화된 API 만 쓰지만
/// 실측에서 호출당 60ms~1.5초(시스템 전체 스레드를 훑는다)가 걸려 쓰지 않는다.
pub(crate) fn resume_process(child: &Child) -> io::Result<()> {
    let status = unsafe { NtResumeProcess(child_handle(child)?) };
    if status < 0 {
        return Err(io::Error::other(format!("NtResumeProcess 실패(NTSTATUS {status:#010x})")));
    }
    Ok(())
}
