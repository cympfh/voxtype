use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use crate::config::{Config, Mode};

/// デスクトップ GUI 側の Config から VR オーバーレイ描画スレッドへ送るスナップショット。
#[derive(Debug, Clone, PartialEq)]
pub struct OverlaySnapshot {
    pub clipboard_enabled: bool,
    pub auto_input_enabled: bool,
    pub auto_input_send_enter: bool,
    pub vrchat_enabled: bool,
    /// 校正(Grok)の ON/OFF
    pub proofread_enabled: bool,
    /// 加工モード(エコー/翻訳/質問)
    pub mode: Mode,
    pub is_recording: bool,
    pub last_transcription: String,
    /// 加工後の出力(フォーマット済み。未加工なら認識結果そのまま)
    pub last_output: String,
    /// 右端の A/B/X/Y ボタン
    pub avatar_buttons: [AvatarButtonView; 4],
}

/// VR に出す A/B/X/Y ボタン1つ分。押せない(未設定)ボタンは薄く出す
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AvatarButtonView {
    pub configured: bool,
    pub on: bool,
}

/// App → オーバーレイのスレッドへの頼みごと(SteamVR を呼べるのはそのスレッドだけ)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayCommand {
    /// SteamVR のキー割り当て画面を PC の画面に開く(設定画面の「キー割り当て変更」)
    OpenBindingUi,
}

impl OverlaySnapshot {
    pub fn from_config(
        config: &Config,
        is_recording: bool,
        last_transcription: &str,
        last_output: &str,
        avatar_states: &crate::avatar_buttons::ToggleStates,
    ) -> Self {
        Self {
            clipboard_enabled: config.clipboard_enabled,
            auto_input_enabled: config.auto_input_enabled,
            auto_input_send_enter: config.auto_input_send_enter,
            vrchat_enabled: config.vrchat_enabled,
            proofread_enabled: config.proofread_enabled,
            mode: config.mode,
            is_recording,
            last_transcription: last_transcription.to_string(),
            last_output: last_output.to_string(),
            avatar_buttons: std::array::from_fn(|i| {
                let b = &config.avatar_buttons[i];
                AvatarButtonView {
                    configured: b.is_configured() && b.error().is_none(),
                    on: avatar_states.0[i],
                }
            }),
        }
    }
}

/// VR オーバーレイに描画する1行分の情報。デスクトップGUI(main.rs)のチェックボックスと
/// 同じラベル・順序・インデント構造を保つ。
pub struct OverlayField {
    pub label: &'static str,
    pub enabled: bool,
    pub indent: bool,
    pub action: OverlayAction,
}

/// デスクトップGUIのチェックボックス群と対応するオーバーレイ表示用フィールド一覧を返す。
/// 先頭4つが「送り先」カード、残りが「加工」カード。
/// 純粋関数なので全OSでテスト可能。
pub fn overlay_fields(snapshot: &OverlaySnapshot) -> Vec<OverlayField> {
    vec![
        OverlayField {
            label: "クリップボードにコピー",
            enabled: snapshot.clipboard_enabled,
            indent: false,
            action: OverlayAction::ToggleClipboard,
        },
        OverlayField {
            label: "アクティブなウィンドウに入力",
            enabled: snapshot.auto_input_enabled,
            indent: false,
            action: OverlayAction::ToggleAutoInput,
        },
        OverlayField {
            label: "入力後に Enter を送る",
            enabled: snapshot.auto_input_send_enter,
            indent: true,
            action: OverlayAction::ToggleAutoInputSendEnter,
        },
        OverlayField {
            label: "VRChat に入力",
            enabled: snapshot.vrchat_enabled,
            indent: false,
            action: OverlayAction::ToggleVrchat,
        },
        OverlayField {
            label: "校正",
            enabled: snapshot.proofread_enabled,
            indent: false,
            action: OverlayAction::ToggleProofread,
        },
        OverlayField {
            label: "エコー",
            enabled: snapshot.mode == Mode::Echo,
            indent: false,
            action: OverlayAction::SetMode(Mode::Echo),
        },
        OverlayField {
            label: "翻訳",
            enabled: snapshot.mode == Mode::Translate,
            indent: false,
            action: OverlayAction::SetMode(Mode::Translate),
        },
        OverlayField {
            label: "質問 (AI)",
            enabled: snapshot.mode == Mode::Question,
            indent: false,
            action: OverlayAction::SetMode(Mode::Question),
        },
    ]
}

/// VR オーバーレイ側のチェックボックス操作 → デスクトップ側 Config への反映アクション。
/// 各バリアントは main.rs の apply_steamvr_action で match され、
/// session.rs の PollNextOverlayEvent 配線(render.rsのクリック検出経由)から construct される。
/// その実際の construct 元である `overlay_fields`/render.rs は、非Windows host build では
/// (テスト以外)呼び出されない(render.rs自体が`#[cfg(windows)]`)ため、dead_code は
/// 常にこのプラットフォームで発生する — enumレベルで明示的に許容する。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayAction {
    ToggleClipboard,
    ToggleAutoInput,
    ToggleAutoInputSendEnter,
    ToggleVrchat,
    /// 校正(Grok)の ON/OFF
    ToggleProofread,
    /// 加工モードを切り替える(エコー/翻訳/質問)
    SetMode(Mode),
    CallQvPen,
    ToggleRecording,
    /// 録音していなければ開始する(SteamVR のボタン割り当て「録音を開始」)
    StartRecording,
    /// オーバーレイの「割り当てを変える」ボタン。オーバーレイのスレッドが自分で処理し、App には送らない
    OpenBindingUi,
    /// 右端の A/B/X/Y ボタン(0〜3)を押した
    ToggleAvatarButton(usize),
}

impl OverlayAction {
    /// SteamVR のボタン割り当て(input_manifest.rs)→ アプリの操作
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn from_input(action: super::input_manifest::InputAction) -> Self {
        use super::input_manifest::InputAction;
        match action {
            InputAction::StartRecording => OverlayAction::StartRecording,
            InputAction::ToggleRecording => OverlayAction::ToggleRecording,
            InputAction::CallQvPen => OverlayAction::CallQvPen,
        }
    }
}

pub struct OverlayHandle {
    pub snapshot_tx: Sender<OverlaySnapshot>,
    pub action_rx: Receiver<OverlayAction>,
    pub command_tx: Sender<OverlayCommand>,
    /// いま SteamVR につながっているか(オーバーレイのスレッドが更新する)
    pub connected: Arc<AtomicBool>,
    /// SteamVR が終わったのでアプリも終了してほしい(オーバーレイのスレッドが立てる)
    pub exit_requested: Arc<AtomicBool>,
    /// VR の処理が落ちてつなぎ直している、などデスクトップ画面に出したい問題(無ければ None)
    pub problem: Arc<std::sync::Mutex<Option<String>>>,
}

impl OverlayHandle {
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::SeqCst)
    }

    pub fn problem(&self) -> Option<String> {
        self.problem.lock().ok().and_then(|p| p.clone())
    }
}

/// SteamVR オーバーレイのスレッドを起動する(Windows では常に Some、それ以外は None)。
/// SteamVR が動いていなくてもよい: スレッドが SteamVR の起動を待ち、来たらつなぐ(`is_connected`)。
/// つないだあとに SteamVR が終わったら `exit_requested` が立つので、アプリは終了する。
/// デスクトップ GUI は周期的にハンドルを poll する(Slint 版では常時タイマーが回っている)。
/// `width_m` はオーバーレイの表示幅(メートル、Config の `steamvr_overlay_width_m`)。
#[cfg(windows)]
pub fn start(initial: OverlaySnapshot, width_m: f32) -> Option<OverlayHandle> {
    super::session::start(initial, crate::config::clamp_overlay_width_m(width_m))
}

#[cfg(not(windows))]
pub fn start(_initial: OverlaySnapshot, _width_m: f32) -> Option<OverlayHandle> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_input_maps_every_steamvr_action() {
        use crate::steamvr::input_manifest::InputAction;
        assert_eq!(
            OverlayAction::from_input(InputAction::StartRecording),
            OverlayAction::StartRecording
        );
        assert_eq!(
            OverlayAction::from_input(InputAction::ToggleRecording),
            OverlayAction::ToggleRecording
        );
        assert_eq!(
            OverlayAction::from_input(InputAction::CallQvPen),
            OverlayAction::CallQvPen
        );
    }

    #[test]
    fn test_overlay_snapshot_from_config_copies_all_fields() {
        let config = Config {
            clipboard_enabled: false,
            auto_input_enabled: true,
            auto_input_send_enter: true,
            vrchat_enabled: false,
            proofread_enabled: true,
            mode: Mode::Translate,
            ..Default::default()
        };

        let snapshot = OverlaySnapshot::from_config(
            &config,
            true,
            "こんにちは",
            "こんにちは/Hello",
            &Default::default(),
        );

        assert_eq!(snapshot.clipboard_enabled, config.clipboard_enabled);
        assert_eq!(snapshot.auto_input_enabled, config.auto_input_enabled);
        assert_eq!(snapshot.auto_input_send_enter, config.auto_input_send_enter);
        assert_eq!(snapshot.vrchat_enabled, config.vrchat_enabled);
        assert_eq!(snapshot.proofread_enabled, config.proofread_enabled);
        assert_eq!(snapshot.mode, config.mode);
        assert!(snapshot.is_recording);
        assert_eq!(snapshot.last_transcription, "こんにちは");
        assert_eq!(snapshot.last_output, "こんにちは/Hello");
    }

    // Windows では本物の見張りスレッドが動き、SteamVR が起動していればつないでしまうので呼ばない
    #[cfg(not(windows))]
    #[test]
    fn test_start_is_none_off_windows() {
        let snapshot =
            OverlaySnapshot::from_config(&Config::default(), false, "", "", &Default::default());
        assert!(start(snapshot, 2.4).is_none());
    }

    fn all_false_snapshot() -> OverlaySnapshot {
        OverlaySnapshot {
            clipboard_enabled: false,
            auto_input_enabled: false,
            auto_input_send_enter: false,
            vrchat_enabled: false,
            proofread_enabled: false,
            mode: Mode::Echo,
            is_recording: false,
            last_transcription: String::new(),
            last_output: String::new(),
            avatar_buttons: Default::default(),
        }
    }

    #[test]
    fn test_snapshot_avatar_buttons_dim_unconfigured_and_invalid() {
        use crate::avatar_buttons::{AvatarButton, ParamType, ToggleStates};
        let mut config = Config::default();
        config.avatar_buttons[0].param = "Hat".into();
        config.avatar_buttons[1] = AvatarButton {
            param: "Bad".into(),
            kind: ParamType::Int,
            on: "999".into(),
            off: "0".into(),
        };
        let s = OverlaySnapshot::from_config(
            &config,
            false,
            "",
            "",
            &ToggleStates([true, true, false, false]),
        );
        let v: Vec<(bool, bool)> = s
            .avatar_buttons
            .iter()
            .map(|b| (b.configured, b.on))
            .collect();
        assert_eq!(
            v,
            vec![(true, true), (false, true), (false, false), (false, false)]
        );
    }

    #[test]
    fn test_overlay_fields_labels_order_and_indent() {
        let fields = overlay_fields(&all_false_snapshot());
        let expected: Vec<(&str, bool, OverlayAction)> = vec![
            (
                "クリップボードにコピー",
                false,
                OverlayAction::ToggleClipboard,
            ),
            (
                "アクティブなウィンドウに入力",
                false,
                OverlayAction::ToggleAutoInput,
            ),
            (
                "入力後に Enter を送る",
                true,
                OverlayAction::ToggleAutoInputSendEnter,
            ),
            ("VRChat に入力", false, OverlayAction::ToggleVrchat),
            ("校正", false, OverlayAction::ToggleProofread),
            ("エコー", false, OverlayAction::SetMode(Mode::Echo)),
            ("翻訳", false, OverlayAction::SetMode(Mode::Translate)),
            ("質問 (AI)", false, OverlayAction::SetMode(Mode::Question)),
        ];
        assert_eq!(fields.len(), expected.len());
        for (field, (label, indent, action)) in fields.iter().zip(expected.iter()) {
            assert_eq!(field.label, *label);
            assert_eq!(field.indent, *indent);
            assert_eq!(field.action, *action);
        }
    }

    #[test]
    fn test_overlay_fields_reflects_each_toggle_flag_independently() {
        let setters: Vec<(fn(&mut OverlaySnapshot), usize)> = vec![
            (|s| s.clipboard_enabled = true, 0),
            (|s| s.auto_input_enabled = true, 1),
            (|s| s.auto_input_send_enter = true, 2),
            (|s| s.vrchat_enabled = true, 3),
            (|s| s.proofread_enabled = true, 4),
        ];

        // モードの3行(5〜7)は別テスト(test_overlay_fields_mode_is_radio)で確かめる
        for (set_flag, expected_index) in setters {
            let mut snapshot = all_false_snapshot();
            set_flag(&mut snapshot);
            let fields = overlay_fields(&snapshot);
            for (i, field) in fields.iter().enumerate().take(5) {
                assert_eq!(
                    field.enabled,
                    i == expected_index,
                    "index {} の enabled が意図しない値: {:?}",
                    i,
                    field.label
                );
            }
        }
    }

    #[test]
    fn test_overlay_fields_mode_is_radio() {
        // モードの3行は今のモードだけが ON になる
        for (mode, on_index) in [(Mode::Echo, 5), (Mode::Translate, 6), (Mode::Question, 7)] {
            let mut snapshot = all_false_snapshot();
            snapshot.mode = mode;
            let fields = overlay_fields(&snapshot);
            for (i, field) in fields.iter().enumerate().skip(5) {
                assert_eq!(
                    field.enabled,
                    i == on_index,
                    "mode {:?} の index {}",
                    mode,
                    i
                );
            }
        }
    }
}
