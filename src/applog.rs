//! 自前のログファイル %APPDATA%\voxtype\voxtype.log。
//! どう起動されても(ダブルクリック・SteamVR の自動起動・WSL)stderr が見えないので、
//! `log!` で出したものは時刻付きでこのファイルにも書く。前回の分は .log.1 に 1 世代だけ残し、
//! 1 回の起動で MAX_BYTES を超えたらそこでも回す。秘密(API キーなど)はここに渡さないこと。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FILE_NAME: &str = "voxtype.log";
const MAX_BYTES: u64 = 1024 * 1024;

struct LogFile {
    path: PathBuf,
    file: File,
    written: u64,
}

static LOG: Mutex<Option<LogFile>> = Mutex::new(None);

/// 起動時に 1 回呼ぶ。失敗してもアプリは動かす(stderr にだけ出る)
pub fn init() {
    let Ok(dir) = crate::config::app_config_dir() else {
        return;
    };
    match open_in(&dir) {
        Ok(f) => *LOG.lock().unwrap_or_else(|e| e.into_inner()) = Some(f),
        Err(e) => to_stderr(&format!("ログファイルを開けない: {}", e)),
    }
}

/// 前回のログを .1 に回して、新しいログを開く
fn open_in(dir: &Path) -> std::io::Result<LogFile> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(FILE_NAME);
    rotate(&path);
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)?;
    Ok(LogFile {
        path,
        file,
        written: 0,
    })
}

fn rotate(path: &Path) {
    if path.exists() {
        let _ = std::fs::rename(path, path.with_extension("log.1"));
    }
}

/// パニックしたら、メッセージと場所(とバックトレース)をログに残す。
/// stderr はふつう誰も見ていないので、これが無いと落ちた理由が分からない
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let thread = std::thread::current();
        let msg = panic_line(
            thread.name().unwrap_or("?"),
            &payload_message(info.payload()),
            info.location()
                .map(|l| format!("{}:{}", l.file(), l.line())),
        );
        let bt = std::backtrace::Backtrace::force_capture();
        let line = format!("{}\n{}", msg, bt);
        to_stderr(&line);
        // ログを書いている最中のパニックで固まらないよう、ロックが取れなければ諦める
        if let Ok(mut guard) = LOG.try_lock()
            && let Some(log) = guard.as_mut()
        {
            let _ = log
                .file
                .write_all(format_line(&now_local(), &line).as_bytes());
        }
    }));
}

/// パニックの中身(`panic!` に渡した文字列)を取り出す
pub fn payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "(不明)".to_string()
    }
}

fn panic_line(thread: &str, msg: &str, location: Option<String>) -> String {
    format!(
        "[PANIC] スレッド {} が落ちた: {} ({})",
        thread,
        msg,
        location.as_deref().unwrap_or("場所不明")
    )
}

/// stderr にも出す。ただし書けなくても無視する。
/// `eprintln!` は書けないとパニックする。WSL から起動すると、しばらくして stderr の中継
/// (パイプ)が消えるので、次の 1 行で SteamVR のスレッドがパニックで死に、オーバーレイが
/// 黙って消えていた(パニックの知らせも同じ stderr に出るので何も残らなかった)
fn to_stderr(line: &str) {
    let _ = writeln!(std::io::stderr(), "{}", line);
}

/// `log!` の中身。stderr と(開いていれば)ログファイルの両方に書く
pub fn write(msg: &str) {
    to_stderr(msg);
    let line = format_line(&now_local(), msg);
    let mut guard = LOG.lock().unwrap_or_else(|e| e.into_inner());
    let Some(log) = guard.as_mut() else {
        return;
    };
    if log.written + line.len() as u64 > MAX_BYTES {
        let dir = log.path.parent().map(Path::to_path_buf);
        *guard = dir.and_then(|d| open_in(&d).ok());
    }
    if let Some(log) = guard.as_mut()
        && log.file.write_all(line.as_bytes()).is_ok()
    {
        log.written += line.len() as u64;
    }
}

/// (年, 月, 日, 時, 分, 秒, ミリ秒)
type Stamp = (u16, u16, u16, u16, u16, u16, u16);

fn format_line(t: &Stamp, msg: &str) -> String {
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} {}\n",
        t.0, t.1, t.2, t.3, t.4, t.5, t.6, msg
    )
}

#[cfg(windows)]
fn now_local() -> Stamp {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    let mut st = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut st) };
    (
        st.wYear,
        st.wMonth,
        st.wDay,
        st.wHour,
        st.wMinute,
        st.wSecond,
        st.wMilliseconds,
    )
}

/// Windows 以外(テスト用)は UTC
#[cfg(not(windows))]
fn now_local() -> Stamp {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let (y, m, day) = civil_from_days((secs / 86400) as i64);
    let s = secs % 86400;
    (
        y as u16,
        m as u16,
        day as u16,
        (s / 3600) as u16,
        (s / 60 % 60) as u16,
        (s % 60) as u16,
        d.subsec_millis() as u16,
    )
}

/// 1970-01-01 からの日数 → (年, 月, 日)(Howard Hinnant のアルゴリズム)
#[cfg(not(windows))]
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

/// `eprintln!` と同じ書き方で、stderr とログファイルに出す
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::applog::write(&format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_panic_line_and_payload() {
        assert_eq!(
            panic_line("steamvr", "boom", Some("src/x.rs:3".into())),
            "[PANIC] スレッド steamvr が落ちた: boom (src/x.rs:3)"
        );
        let p = std::panic::catch_unwind(|| panic!("n={}", 5)).unwrap_err();
        assert_eq!(payload_message(p.as_ref()), "n=5");
        let p = std::panic::catch_unwind(|| panic!("static")).unwrap_err();
        assert_eq!(payload_message(p.as_ref()), "static");
    }

    #[test]
    fn test_write_without_log_file_does_not_panic() {
        write("ログファイルなしでも落ちない");
    }

    #[test]
    fn test_format_line() {
        let line = format_line(&(2026, 10, 20, 22, 5, 9, 7), "[SteamVR] つないだ");
        assert_eq!(line, "2026-10-20 22:05:09.007 [SteamVR] つないだ\n");
    }

    #[test]
    fn test_open_in_keeps_previous_run_as_dot_1() {
        let dir = std::env::temp_dir().join(format!("voxtype-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut first = open_in(&dir).unwrap();
        first.file.write_all(b"run1\n").unwrap();
        drop(first);
        let mut second = open_in(&dir).unwrap();
        second.file.write_all(b"run2\n").unwrap();
        drop(second);
        assert_eq!(
            std::fs::read_to_string(dir.join(FILE_NAME)).unwrap(),
            "run2\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("voxtype.log.1")).unwrap(),
            "run1\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(not(windows))]
    #[test]
    fn test_civil_from_days() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20746), (2026, 10, 20));
    }
}
