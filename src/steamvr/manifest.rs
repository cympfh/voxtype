//! SteamVR起動時にこのexeを自動起動してもらうためのOpenVRアプリケーションマニフェスト
//! (`.vrmanifest`)の組み立て・書き込み。
//!
//! 実際の`IVRApplications::AddApplicationManifest`/`SetApplicationAutoLaunch`呼び出しは
//! session.rs(Windows専用)側が担う。ここではファイル内容の組み立てとディスクへの
//! 書き込みのみを扱う純粋度の高いロジックであり、Windows専用のffi.rsとは独立させて
//! non-Windows host(WSL)でもユニットテストできるようにしている。

use std::path::{Path, PathBuf};

/// ダッシュボードオーバーレイの`CreateDashboardOverlay`キー(session.rsが`create_dashboard_overlay`
/// に渡す値)と一致させる必要がある — OpenVR仕様上、オーバーレイのキーと登録アプリの
/// app_keyが一致していないとSteamVR側で同一アプリとして関連付けられない。
pub const APP_KEY: &str = "cympfh.vrc_companion";
const APP_NAME: &str = "VRC Companion";
const MANIFEST_FILE_NAME: &str = "voxtype.vrmanifest";
/// 0.10 までのマニフェスト名。SteamVR の旧登録を外すときに使う
#[cfg_attr(not(windows), allow(dead_code))] // session.rs が Windows だけで参照する
pub const LEGACY_MANIFEST_FILE_NAME: &str = "vrc-companion.vrmanifest";
const ICON_FILE_NAME: &str = "vrc-companion-icon.png";
/// ロゴ(assets/logo.png, 256x256)。SteamVR のアプリ一覧・ダッシュボードのタブのアイコンに使う
pub const ICON_PNG: &[u8] = include_bytes!("../../assets/logo.png");

/// OpenVRのアプリケーションマニフェストJSONを組み立てる純粋関数。
/// `action_manifest_path` は SteamVR Input のアクションマニフェスト(input_manifest.rs)。
/// これがあると、アプリが起動していなくても SteamVR のバインド画面にこのアプリが出る。
pub fn build_manifest_json(
    app_key: &str,
    name: &str,
    exe_path: &str,
    action_manifest_path: Option<&str>,
    image_path: Option<&str>,
) -> String {
    let mut json = serde_json::json!({
        "source": "custom",
        "applications": [
            {
                "app_key": app_key,
                "launch_type": "binary",
                "binary_path_windows": exe_path,
                "is_dashboard_overlay": true,
                "strings": {
                    "en_us": {
                        "name": name
                    }
                }
            }
        ]
    });
    if let Some(path) = action_manifest_path {
        json["applications"][0]["action_manifest_path"] = path.into();
    }
    if let Some(path) = image_path {
        json["applications"][0]["image_path"] = path.into();
    }
    json.to_string()
}

/// アイコン PNG の置き場所(`dir` は %APPDATA%\voxtype)
pub fn icon_path_in(dir: &Path) -> PathBuf {
    dir.join(ICON_FILE_NAME)
}

/// アイコン PNG を `dir` に書き、そのフルパスを返す。SteamVR はファイルのパスでしか受け取らない
pub fn write_icon_in(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("config dir作成失敗: {}", e))?;
    let path = icon_path_in(dir);
    // 同じ中身ならそのまま(起動ごとにマニフェストとタブの両方から呼ばれる)
    if std::fs::read(&path).is_ok_and(|b| b == ICON_PNG) {
        return Ok(path);
    }
    if let Err(e) = std::fs::write(&path, ICON_PNG) {
        // 書けなくても前のファイルが残っていればそれを使う
        if path.exists() {
            crate::log!("[SteamVR] アイコン更新失敗(前のファイルを使う): {}", e);
            return Ok(path);
        }
        return Err(format!("アイコン書き込み失敗: {}", e));
    }
    Ok(path)
}

/// マニフェストを自身の config dir(%APPDATA%\voxtype)に書き込み、そのフルパスを返す。
/// session.rs(Windows専用)からのみ呼ばれるため、non-Windows host build では dead_code になる。
#[cfg_attr(not(windows), allow(dead_code))]
pub fn write_manifest(action_manifest_path: Option<&Path>) -> Result<PathBuf, String> {
    let exe_path = std::env::current_exe().map_err(|e| format!("current_exe取得失敗: {}", e))?;
    let config_dir = crate::config::app_config_dir()?;
    write_manifest_in(
        &config_dir,
        &exe_path.to_string_lossy(),
        action_manifest_path,
    )
}

/// `dir` にマニフェストを書く(テストは一時ディレクトリに書く。本物のファイルには触らない)
fn write_manifest_in(
    dir: &Path,
    exe_path: &str,
    action_manifest_path: Option<&Path>,
) -> Result<PathBuf, String> {
    let action_manifest_path = action_manifest_path.map(|p| p.to_string_lossy().into_owned());
    let image_path = match write_icon_in(dir) {
        Ok(p) => Some(p.to_string_lossy().into_owned()),
        Err(e) => {
            crate::log!("[SteamVR] {}", e);
            None
        }
    };
    let json = build_manifest_json(
        APP_KEY,
        APP_NAME,
        exe_path,
        action_manifest_path.as_deref(),
        image_path.as_deref(),
    );
    std::fs::create_dir_all(dir).map_err(|e| format!("config dir作成失敗: {}", e))?;
    let manifest_path = dir.join(MANIFEST_FILE_NAME);
    std::fs::write(&manifest_path, json).map_err(|e| format!("マニフェスト書き込み失敗: {}", e))?;
    Ok(manifest_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_icon_path_and_rewrite_skipped_when_identical() {
        let dir = std::env::temp_dir().join(format!("voxtype-icon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = write_icon_in(&dir).unwrap();
        assert_eq!(path, dir.join("vrc-companion-icon.png"));
        assert_eq!(path, icon_path_in(&dir));
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert_eq!(write_icon_in(&dir).unwrap(), path);
        assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_icon_png_is_256_square_png() {
        assert_eq!(&ICON_PNG[..8], b"\x89PNG\r\n\x1a\n");
        let w = u32::from_be_bytes(ICON_PNG[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(ICON_PNG[20..24].try_into().unwrap());
        assert_eq!((w, h), (256, 256));
    }

    #[test]
    fn test_build_manifest_json_contains_app_key_name_and_path() {
        let json = build_manifest_json(
            "test.app_key",
            "Test App",
            r"C:\path\to\app.exe",
            None,
            None,
        );
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let app = &parsed["applications"][0];
        assert_eq!(app["app_key"], "test.app_key");
        assert_eq!(app["launch_type"], "binary");
        assert_eq!(app["binary_path_windows"], r"C:\path\to\app.exe");
        assert_eq!(app["is_dashboard_overlay"], true);
        assert_eq!(app["strings"]["en_us"]["name"], "Test App");
        assert!(app.get("action_manifest_path").is_none());
        assert!(app.get("image_path").is_none());
    }

    #[test]
    fn test_build_manifest_json_includes_action_manifest_path() {
        let json = build_manifest_json(
            "test.app_key",
            "Test App",
            r"C:\path\to\app.exe",
            Some(r"C:\cfg\steamvr_input\actions.json"),
            Some(r"C:\cfg\icon.png"),
        );
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed["applications"][0]["action_manifest_path"],
            r"C:\cfg\steamvr_input\actions.json"
        );
        assert_eq!(parsed["applications"][0]["image_path"], r"C:\cfg\icon.png");
    }

    #[test]
    fn test_write_manifest_writes_file_with_own_app_key() {
        // 本物の %APPDATA%\voxtype\voxtype.vrmanifest には触らない
        // (以前は本物を書いて消していたので、Windows でテストを走らせると SteamVR の自動起動が外れた)
        let dir =
            std::env::temp_dir().join(format!("voxtype-manifest-test-{}", std::process::id()));
        let path = write_manifest_in(&dir, r"C:\path\to\app.exe", None).unwrap();
        assert!(path.starts_with(&dir));
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["applications"][0]["app_key"], APP_KEY);
        let icon = parsed["applications"][0]["image_path"].as_str().unwrap();
        assert_eq!(std::fs::read(icon).unwrap(), ICON_PNG);
        std::fs::remove_dir_all(&dir).ok();
    }
}
