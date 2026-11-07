//! SteamVR が動いているかの見張り。
//!
//! このアプリが SteamVR を起動してしまわないよう、`VR_Init` は vrserver.exe が動いているときだけ
//! 呼ぶ(SteamVR が動いていないときに Overlay として `VR_Init` すると、SteamVR が起動してしまう)。
//! 動いていなければ数秒おきに見に行き、SteamVR が来たらつなぐ。
//! つないだあとに SteamVR が終わっても(終了の知らせ、または vrserver.exe が居なくなっても)、
//! PC 側は終了せず切断して待機に戻る。SteamVR が起動したら自動でつなぎ直す。
//! 判定の部分は描画先に依存しないので全OSでテストできる。

use std::time::{Duration, Instant};

/// SteamVR が動いている間だけ居るプロセス
pub const STEAMVR_SERVER_EXE: &str = "vrserver.exe";
/// SteamVR が動いていない・つなげなかったときに、もう一度見に行く間隔
pub const RETRY_EVERY: Duration = Duration::from_secs(5);
/// つながっている間に vrserver.exe が居なくなっていないかを見る間隔
pub const ALIVE_CHECK_EVERY: Duration = Duration::from_secs(1);

pub fn is_steamvr_server(process_name: &str) -> bool {
    process_name.eq_ignore_ascii_case(STEAMVR_SERVER_EXE)
}

/// 接続が終わったあとにすること
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterSession {
    /// アプリ(デスクトップの画面も)を終了する
    ExitApp,
    /// SteamVR はまだ動いているので、少し待ってつなぎ直す
    Retry,
    /// アプリがもう閉じている。見張りのスレッドも終わる
    ThreadDone,
}

/// つながっていた SteamVR との接続が終わった理由
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEnd {
    /// アプリ本体が閉じた(スレッドも終わる)
    AppClosed,
    /// SteamVR から終了の知らせ(VREvent_Quit)が来た
    SteamVrQuit,
    /// 知らせなしに vrserver.exe が居なくなった(落ちた・強制終了など)
    SteamVrGone,
    /// このアプリの別のコピーがもう SteamVR にタブを出している(例: 手で起動したあとに
    /// SteamVR の自動起動でもう1つ起動された)。このコピーは終了する
    AnotherInstance,
    /// つないだあとの準備(オーバーレイ作成など)に失敗した
    Failed(String),
}

impl SessionEnd {
    pub fn after(&self) -> AfterSession {
        match self {
            SessionEnd::AppClosed => AfterSession::ThreadDone,
            // SteamVR が終わった/居なくなっただけなら切断して待機に戻る
            // (PC 側は単体で使えるツールなので終了しない)
            SessionEnd::SteamVrQuit | SessionEnd::SteamVrGone => AfterSession::Retry,
            SessionEnd::AnotherInstance => AfterSession::ExitApp,
            SessionEnd::Failed(_) => AfterSession::Retry,
        }
    }

    pub fn message(&self) -> String {
        match self {
            SessionEnd::AppClosed => "アプリを閉じる".to_string(),
            SessionEnd::AnotherInstance => {
                "このアプリの別のコピーがもう SteamVR につないでいるので、こちらは終了する"
                    .to_string()
            }
            SessionEnd::SteamVrQuit => "SteamVR が終了した。起動したらまた自動でつなぐ".to_string(),
            SessionEnd::SteamVrGone => {
                "SteamVR(vrserver.exe)が居なくなった。起動したらまた自動でつなぐ".to_string()
            }
            SessionEnd::Failed(e) => format!(
                "準備に失敗したので切断した({}秒後にやり直す): {}",
                RETRY_EVERY.as_secs(),
                e
            ),
        }
    }
}

/// 同じ内容を何度もログに出さない(数秒おきにやり直すので)
#[derive(Debug, Default)]
pub struct StatusLog {
    last: Option<String>,
}

impl StatusLog {
    /// 前回と違うときだけ true(= ログに出す)
    pub fn changed(&mut self, message: &str) -> bool {
        if self.last.as_deref() == Some(message) {
            return false;
        }
        self.last = Some(message.to_string());
        true
    }

    pub fn log(&mut self, message: &str) {
        if self.changed(message) {
            crate::log!("[SteamVR] {}", message);
        }
    }
}

/// 同じ種類のエラーを `every` に 1 回だけログに出す(その間に何回起きたかは次の行に添える)
pub struct RateLimited {
    every: Duration,
    last: Option<Instant>,
    suppressed: u32,
}

impl RateLimited {
    pub fn new(every: Duration) -> Self {
        Self {
            every,
            last: None,
            suppressed: 0,
        }
    }

    /// 出すべきなら、出す文を返す(`now` はテストのため外から渡す)
    pub fn check(&mut self, now: Instant, message: &str) -> Option<String> {
        if let Some(last) = self.last
            && now.duration_since(last) < self.every
        {
            self.suppressed += 1;
            return None;
        }
        self.last = Some(now);
        let n = std::mem::take(&mut self.suppressed);
        Some(if n == 0 {
            message.to_string()
        } else {
            format!("{} (この間ほかに {} 回)", message, n)
        })
    }
}

/// vrserver.exe が動いているか(プロセス一覧を見るだけで、SteamVR には触らない)
#[cfg(windows)]
pub fn steamvr_running() -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut found = false;
        let mut ok = Process32FirstW(snap, &mut entry) != 0;
        while ok {
            let len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            if is_steamvr_server(&String::from_utf16_lossy(&entry.szExeFile[..len])) {
                found = true;
                break;
            }
            ok = Process32NextW(snap, &mut entry) != 0;
        }
        CloseHandle(snap);
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_vrserver_counts_as_steamvr_running() {
        assert!(is_steamvr_server("vrserver.exe"));
        assert!(is_steamvr_server("VRServer.EXE"));
        // vrmonitor やスタートアップだけ残っていても、つなぎに行かない
        assert!(!is_steamvr_server("vrmonitor.exe"));
        assert!(!is_steamvr_server("vrstartup.exe"));
        assert!(!is_steamvr_server("vrserver.exe.bak"));
        assert!(!is_steamvr_server("vrc-companion.exe"));
    }

    #[test]
    fn status_log_reports_only_changes() {
        let mut log = StatusLog::default();
        assert!(log.changed("SteamVR の起動を待っています"));
        assert!(!log.changed("SteamVR の起動を待っています"));
        assert!(log.changed("接続した"));
        assert!(log.changed("SteamVR の起動を待っています"));
    }

    #[test]
    fn rate_limited_logs_first_then_once_per_period_with_count() {
        let t0 = Instant::now();
        let mut r = RateLimited::new(Duration::from_secs(60));
        assert_eq!(r.check(t0, "e").as_deref(), Some("e"));
        assert_eq!(r.check(t0 + Duration::from_secs(1), "e"), None);
        assert_eq!(r.check(t0 + Duration::from_secs(2), "e"), None);
        assert_eq!(
            r.check(t0 + Duration::from_secs(61), "e").as_deref(),
            Some("e (この間ほかに 2 回)")
        );
        assert_eq!(
            r.check(t0 + Duration::from_secs(200), "e").as_deref(),
            Some("e")
        );
    }

    #[test]
    fn session_end_messages_say_what_happens_next() {
        assert!(SessionEnd::SteamVrQuit.message().contains("つなぐ"));
        assert!(SessionEnd::SteamVrGone.message().contains("つなぐ"));
        assert!(SessionEnd::AnotherInstance.message().contains("終了する"));
        assert!(
            SessionEnd::Failed("x".into())
                .message()
                .contains("やり直す")
        );
    }

    #[test]
    fn steamvr_ending_exits_the_app_but_setup_failure_retries() {
        // SteamVR が終わったら(知らせあり・なし)アプリも終了。重複したコピーも終了
        assert_eq!(SessionEnd::SteamVrQuit.after(), AfterSession::Retry);
        assert_eq!(SessionEnd::SteamVrGone.after(), AfterSession::Retry);
        assert_eq!(SessionEnd::AnotherInstance.after(), AfterSession::ExitApp);
        // SteamVR は動いたままの準備失敗はつなぎ直す
        assert_eq!(SessionEnd::Failed("x".into()).after(), AfterSession::Retry);
        // アプリがもう閉じているなら見張りも終わるだけ
        assert_eq!(SessionEnd::AppClosed.after(), AfterSession::ThreadDone);
    }

    #[test]
    fn retry_is_every_few_seconds() {
        assert!(RETRY_EVERY >= Duration::from_secs(2) && RETRY_EVERY <= Duration::from_secs(10));
        assert!(ALIVE_CHECK_EVERY < RETRY_EVERY);
    }
}
