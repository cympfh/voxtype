//! SteamVR Input(IVRInput)用のアクションマニフェストと既定バインドの組み立て・書き込み。
//!
//! 「どのボタンで何をするか」をアプリ側に固定で持たず、SteamVR の
//! 「コントローラーのバインド」画面でユーザーが自由に割り当てられるようにするためのもの。
//! ここはファイル内容を組み立てて書くだけの純粋なロジックで、non-Windows host でも
//! テストできる(実際の `SetActionManifestPath` などの呼び出しは session.rs 側)。
//!
//! 既定バインドは「VRChat のマイク(ミュート)ボタンをダブルクリック」(0.4 までは VRChat の
//! OSC で MuteSelf を見て同じ操作を検知していたが、その仕組みは 0.5 で削除した)。
//! VRChat 側もそのボタンを2回押されたことになるので、ミュートは2回切り替わって元に戻る。
//! - Index (knuckles): VRChat の既定のマイクは左 A
//! - Quest (oculus_touch): 左 X
//! - Steam Frame (frame_controller): VRChat は Frame 用の既定バインドを持たず、SteamVR が
//!   oculus_touch / knuckles のものを変換して使う。どちらも左 X / 左 A → 左 dpad_down になる
//!   (SteamVR の drivers/frame_controller/resources/input/frame_controller_remapping.json)

use std::path::{Path, PathBuf};

pub const ACTION_SET: &str = "/actions/main";

/// アプリが受け取るボタン操作。SteamVR のバインド画面に並ぶ単位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputAction {
    /// 録音開始(録音中・文字起こし中は何もしない)。従来の「ミュート2回」と同じ
    StartRecording,
    /// 録音の開始/停止を切り替える(オーバーレイの Start/Stop ボタンと同じ)
    ToggleRecording,
    /// QvPen を呼ぶ(オーバーレイの QvPen ボタンと同じ)
    CallQvPen,
}

impl InputAction {
    pub const ALL: [InputAction; 3] = [
        InputAction::StartRecording,
        InputAction::ToggleRecording,
        InputAction::CallQvPen,
    ];

    pub fn path(self) -> &'static str {
        match self {
            InputAction::StartRecording => "/actions/main/in/start_recording",
            InputAction::ToggleRecording => "/actions/main/in/toggle_recording",
            InputAction::CallQvPen => "/actions/main/in/call_qvpen",
        }
    }

    fn label_en(self) -> &'static str {
        match self {
            InputAction::StartRecording => "Start recording",
            InputAction::ToggleRecording => "Start / stop recording",
            InputAction::CallQvPen => "Call QvPen",
        }
    }

    pub fn label_ja(self) -> &'static str {
        match self {
            InputAction::StartRecording => "録音を開始",
            InputAction::ToggleRecording => "録音の開始/停止",
            InputAction::CallQvPen => "QvPen を呼ぶ",
        }
    }
}

pub const ACTION_MANIFEST_FILE_NAME: &str = "actions.json";

/// (controller_type, 既定バインドのファイル名, 「録音を開始」をダブルクリックで割り当てるボタン)
pub const DEFAULT_BINDINGS: [(&str, &str, &str); 3] = [
    (
        "frame_controller",
        "bindings_frame_controller.json",
        "/user/hand/left/input/dpad_down",
    ),
    (
        "knuckles",
        "bindings_knuckles.json",
        "/user/hand/left/input/a",
    ),
    (
        "oculus_touch",
        "bindings_oculus_touch.json",
        "/user/hand/left/input/x",
    ),
];

/// ヘッドセット本体用の空のバインド。無いと SteamVR のログに
/// 「(frame_hmd) has no configured binding」が出続ける。何も割り当てないので害はない
pub const EMPTY_BINDINGS: [(&str, &str); 1] = [("frame_hmd", "bindings_frame_hmd.json")];

pub fn build_action_manifest_json() -> String {
    let default_bindings: Vec<_> = DEFAULT_BINDINGS
        .iter()
        .map(|(controller_type, file, _)| (*controller_type, *file))
        .chain(EMPTY_BINDINGS.iter().copied())
        .map(|(controller_type, file)| {
            serde_json::json!({ "controller_type": controller_type, "binding_url": file })
        })
        .collect();
    let actions: Vec<_> = InputAction::ALL
        .iter()
        .map(|a| serde_json::json!({ "name": a.path(), "type": "boolean", "requirement": "optional" }))
        .collect();

    let mut en = serde_json::Map::new();
    let mut ja = serde_json::Map::new();
    en.insert("language_tag".into(), "en_US".into());
    ja.insert("language_tag".into(), "ja_JP".into());
    en.insert(ACTION_SET.into(), "VRC Companion".into());
    ja.insert(ACTION_SET.into(), "VRC Companion".into());
    for a in InputAction::ALL {
        en.insert(a.path().into(), a.label_en().into());
        ja.insert(a.path().into(), a.label_ja().into());
    }

    serde_json::to_string_pretty(&serde_json::json!({
        "default_bindings": default_bindings,
        "actions": actions,
        "action_sets": [ { "name": ACTION_SET, "usage": "single" } ],
        "localization": [ en, ja ],
    }))
    .unwrap()
}

/// 1台のコントローラー用の既定バインド。`button_path` のダブルクリックで「録音を開始」。
/// 他のアクションは既定では割り当てない(ユーザーが SteamVR の画面で割り当てる)
pub fn build_binding_json(controller_type: &str, button_path: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "action_manifest_version": 0,
        "alias_info": {},
        "app_key": super::manifest::APP_KEY,
        "bindings": {
            ACTION_SET: {
                "chords": [],
                "haptics": [],
                "poses": [],
                "skeleton": [],
                "sources": [
                    {
                        "inputs": {
                            "double": { "output": InputAction::StartRecording.path() }
                        },
                        "mode": "button",
                        "parameters": {},
                        "path": button_path
                    }
                ]
            }
        },
        "controller_type": controller_type,
        "description": "VRChat のマイクボタンをダブルクリックで録音開始",
        "name": "VRC Companion default",
        "options": {},
        "simulated_actions": []
    }))
    .unwrap()
}

/// 何も割り当てないバインド(ヘッドセット本体用)
pub fn build_empty_binding_json(controller_type: &str) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "action_manifest_version": 0,
        "alias_info": {},
        "app_key": super::manifest::APP_KEY,
        "bindings": {
            ACTION_SET: { "chords": [], "haptics": [], "poses": [], "skeleton": [], "sources": [] }
        },
        "controller_type": controller_type,
        "description": "割り当てなし",
        "name": "VRC Companion default",
        "options": {},
        "simulated_actions": []
    }))
    .unwrap()
}

/// アクションマニフェストと既定バインドを `dir` に書き、マニフェストのフルパスを返す。
/// 毎回上書きする(ユーザーが SteamVR で変えたバインドは SteamVR 側に保存されるので消えない)
pub fn write_input_files(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("フォルダ作成失敗: {}", e))?;
    for (controller_type, file, button) in DEFAULT_BINDINGS {
        std::fs::write(dir.join(file), build_binding_json(controller_type, button))
            .map_err(|e| format!("{} の書き込み失敗: {}", file, e))?;
    }
    for (controller_type, file) in EMPTY_BINDINGS {
        std::fs::write(dir.join(file), build_empty_binding_json(controller_type))
            .map_err(|e| format!("{} の書き込み失敗: {}", file, e))?;
    }
    let manifest_path = dir.join(ACTION_MANIFEST_FILE_NAME);
    std::fs::write(&manifest_path, build_action_manifest_json())
        .map_err(|e| format!("アクションマニフェストの書き込み失敗: {}", e))?;
    Ok(manifest_path)
}

/// %APPDATA%\voxtype\steamvr_input
#[allow(dead_code)]
pub fn default_input_dir() -> Result<PathBuf, String> {
    Ok(crate::config::app_config_dir()?.join("steamvr_input"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_manifest_lists_every_action_and_binding_file() {
        let v: serde_json::Value = serde_json::from_str(&build_action_manifest_json()).unwrap();
        let names: Vec<&str> = v["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap())
            .collect();
        for a in InputAction::ALL {
            assert!(names.contains(&a.path()), "{} がない", a.path());
            assert!(a.path().starts_with(&format!("{}/in/", ACTION_SET)));
        }
        assert!(
            v["actions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|a| a["type"] == "boolean")
        );
        assert_eq!(v["action_sets"][0]["name"], ACTION_SET);
        let urls: Vec<&str> = v["default_bindings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["binding_url"].as_str().unwrap())
            .collect();
        assert_eq!(urls.len(), DEFAULT_BINDINGS.len() + EMPTY_BINDINGS.len());
        assert!(urls.contains(&"bindings_frame_hmd.json"));
        for (_, file, _) in DEFAULT_BINDINGS {
            assert!(urls.contains(&file));
        }
        for loc in v["localization"].as_array().unwrap() {
            for a in InputAction::ALL {
                assert!(loc[a.path()].is_string());
            }
        }
    }

    #[test]
    fn test_binding_double_clicks_mic_button_to_start_recording() {
        let v: serde_json::Value = serde_json::from_str(&build_binding_json(
            "frame_controller",
            "/user/hand/left/input/dpad_down",
        ))
        .unwrap();
        assert_eq!(v["controller_type"], "frame_controller");
        assert_eq!(v["app_key"], super::super::manifest::APP_KEY);
        let src = &v["bindings"][ACTION_SET]["sources"][0];
        assert_eq!(src["mode"], "button");
        assert_eq!(src["path"], "/user/hand/left/input/dpad_down");
        assert_eq!(
            src["inputs"]["double"]["output"],
            InputAction::StartRecording.path()
        );
    }

    #[test]
    fn test_empty_binding_has_no_sources() {
        let v: serde_json::Value =
            serde_json::from_str(&build_empty_binding_json("frame_hmd")).unwrap();
        assert_eq!(v["controller_type"], "frame_hmd");
        assert_eq!(
            v["bindings"][ACTION_SET]["sources"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn test_write_input_files_writes_manifest_and_bindings() {
        let dir = std::env::temp_dir().join(format!("voxtype-input-test-{}", std::process::id()));
        let manifest = write_input_files(&dir).unwrap();
        assert_eq!(manifest, dir.join(ACTION_MANIFEST_FILE_NAME));
        assert!(manifest.is_file());
        let files = DEFAULT_BINDINGS
            .iter()
            .map(|b| b.1)
            .chain(EMPTY_BINDINGS.iter().map(|b| b.1));
        for file in files {
            let s = std::fs::read_to_string(dir.join(file)).unwrap();
            serde_json::from_str::<serde_json::Value>(&s).unwrap();
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
