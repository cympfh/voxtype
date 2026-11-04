use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// 認識結果の加工モード(排他)。パイプラインの【モード選択】に対応する
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// そのまま出力
    #[default]
    Echo,
    /// Grok で翻訳してから出力
    Translate,
    /// Grok (QuestionAgent) に質問して回答を出力
    Question,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Echo, Mode::Translate, Mode::Question];

    pub fn label(&self) -> &'static str {
        match self {
            Mode::Echo => "エコー（そのまま）",
            Mode::Translate => "翻訳",
            Mode::Question => "質問 (AI)",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub xai_api_key: String,
    #[serde(default)]
    pub input_device_name: Option<String>,
    #[serde(default = "default_silence_duration_secs")]
    pub silence_duration_secs: f32,
    #[serde(default = "default_silence_threshold")]
    pub silence_threshold: f32,
    #[serde(default = "default_clipboard_enabled")]
    pub clipboard_enabled: bool,
    #[serde(default = "default_auto_input_enabled")]
    pub auto_input_enabled: bool,
    #[serde(default = "default_auto_input_send_enter")]
    pub auto_input_send_enter: bool,
    #[serde(default = "default_vrchat_enabled")]
    pub vrchat_enabled: bool,
    /// 校正(Grok でフィラー除去・言い間違い修正)。翻訳モードでは翻訳の前にかかる。
    /// 質問モードではかけない(質問のプロンプトが音声認識ノイズを前提にしているため)
    #[serde(default)]
    pub proofread_enabled: bool,
    /// 加工モード(エコー/翻訳/質問)
    #[serde(default)]
    pub mode: Mode,
    #[serde(default = "default_translate_lang_preset")]
    pub translate_lang_preset: String,
    #[serde(default = "default_translate_lang_custom")]
    pub translate_lang_custom: String,
    /// 翻訳モードの出力テンプレート。`{source}` `{translated}` が使える
    #[serde(default = "default_translate_format")]
    pub translate_format: String,
    /// 質問モードの出力テンプレート。`{source}` `{answer}` が使える
    #[serde(default = "default_question_format")]
    pub question_format: String,
    /// SteamVR オーバーレイの表示幅(メートル)。起動時に読む
    #[serde(default = "default_steamvr_overlay_width_m")]
    pub steamvr_overlay_width_m: f32,
    /// VR パネル右端の A/B/X/Y ボタン(アバターパラメーター)
    #[serde(default = "crate::avatar_buttons::default_buttons")]
    pub avatar_buttons: [crate::avatar_buttons::AvatarButton; 4],
}

fn default_silence_duration_secs() -> f32 {
    2.0
}

fn default_silence_threshold() -> f32 {
    0.01
}

fn default_clipboard_enabled() -> bool {
    true
}

fn default_auto_input_enabled() -> bool {
    false
}

fn default_auto_input_send_enter() -> bool {
    false
}

fn default_vrchat_enabled() -> bool {
    false
}

fn default_translate_lang_preset() -> String {
    "EN".to_string()
}

fn default_translate_lang_custom() -> String {
    String::new()
}

fn default_translate_format() -> String {
    "{source}/{translated}".to_string()
}

fn default_question_format() -> String {
    "> {source}\nAI> {answer}".to_string()
}

/// 以前は指定なし(SteamVR の既定 1m、UI は 400px 幅に 14px の文字)。
/// 1.92m(0.5.1 までの 2.4m の 80%)。UI は 600px 幅・16〜17px の文字
fn default_steamvr_overlay_width_m() -> f32 {
    1.92
}

/// おかしな値(0、負、巨大)で SteamVR に渡さないように丸める
#[cfg_attr(not(windows), allow(dead_code))]
pub fn clamp_overlay_width_m(w: f32) -> f32 {
    if w.is_finite() {
        w.clamp(0.5, 6.0)
    } else {
        default_steamvr_overlay_width_m()
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            xai_api_key: String::new(),
            input_device_name: None,
            silence_duration_secs: default_silence_duration_secs(),
            silence_threshold: default_silence_threshold(),
            clipboard_enabled: default_clipboard_enabled(),
            auto_input_enabled: default_auto_input_enabled(),
            auto_input_send_enter: default_auto_input_send_enter(),
            vrchat_enabled: default_vrchat_enabled(),
            proofread_enabled: false,
            mode: Mode::default(),
            translate_lang_preset: default_translate_lang_preset(),
            translate_lang_custom: default_translate_lang_custom(),
            translate_format: default_translate_format(),
            question_format: default_question_format(),
            steamvr_overlay_width_m: default_steamvr_overlay_width_m(),
            avatar_buttons: crate::avatar_buttons::default_buttons(),
        }
    }
}

impl Config {
    pub fn config_path() -> Result<PathBuf, String> {
        let config_dir = dirs::config_dir().ok_or("Failed to get config directory")?;
        let app_config_dir = config_dir.join("vrc-companion");

        if !app_config_dir.exists() {
            fs::create_dir_all(&app_config_dir)
                .map_err(|e| format!("Failed to create config directory: {}", e))?;
        }

        Ok(app_config_dir.join("config.json"))
    }

    pub fn load() -> Self {
        match Self::config_path() {
            Ok(path) if path.exists() => match fs::read_to_string(&path) {
                Ok(content) => Self::migrate_and_parse(&content).unwrap_or_else(|e| {
                    crate::log!("Failed to parse config: {}", e);
                    Self::default()
                }),
                Err(e) => {
                    crate::log!("Failed to read config file: {}", e);
                    Self::default()
                }
            },
            Ok(_) => Self::default(),
            Err(e) => {
                crate::log!("Failed to get config path: {}", e);
                Self::default()
            }
        }
    }

    /// 0.6 までの設定ファイルを読む。
    /// - `auto_translate_enabled: true` だった → mode = translate
    /// - eliza_* は削除(知らないキーは serde が読み飛ばす)
    fn migrate_and_parse(content: &str) -> Result<Self, String> {
        let mut value: serde_json::Value =
            serde_json::from_str(content).map_err(|e| e.to_string())?;
        if let Some(obj) = value.as_object_mut() {
            let old_translate = obj
                .get("auto_translate_enabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if old_translate && !obj.contains_key("mode") {
                obj.insert("mode".to_string(), serde_json::json!("translate"));
            }
            obj.remove("auto_translate_enabled");
            obj.remove("eliza_enabled");
            obj.remove("eliza_response_to_vrchat_enabled");
            obj.remove("eliza_url");
        }
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path()?;
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        fs::write(&path, json).map_err(|e| format!("Failed to write config file: {}", e))?;
        Ok(())
    }

    /// auto_input と vrchat は排他。一方を有効にしたらもう一方を無効化する。
    pub fn enable_auto_input_exclusive(&mut self) {
        self.auto_input_enabled = true;
        self.vrchat_enabled = false;
    }

    pub fn enable_vrchat_exclusive(&mut self) {
        self.vrchat_enabled = true;
        self.auto_input_enabled = false;
    }

    /// 翻訳先言語の表示名を preset/custom から解決する
    pub fn translate_target_lang(&self) -> String {
        match self.translate_lang_preset.as_str() {
            "EN" => "英語".to_string(),
            "CN" => "中国語".to_string(),
            _ => self.translate_lang_custom.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.silence_duration_secs, 2.0);
        assert_eq!(config.silence_threshold, 0.01);
        assert!(config.clipboard_enabled);
        assert!(!config.auto_input_enabled);
        assert!(!config.vrchat_enabled);
        assert!(!config.proofread_enabled);
        assert_eq!(config.mode, Mode::Echo);
    }

    #[test]
    fn test_serde_roundtrip() {
        let config = Config {
            xai_api_key: "secret".to_string(),
            vrchat_enabled: true,
            proofread_enabled: true,
            mode: Mode::Translate,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let restored: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.xai_api_key, "secret");
        assert!(restored.vrchat_enabled);
        assert!(restored.proofread_enabled);
        assert_eq!(restored.mode, Mode::Translate);
    }

    #[test]
    fn test_mode_serializes_lowercase() {
        assert_eq!(serde_json::to_string(&Mode::Echo).unwrap(), "\"echo\"");
        assert_eq!(
            serde_json::to_string(&Mode::Translate).unwrap(),
            "\"translate\""
        );
        assert_eq!(
            serde_json::to_string(&Mode::Question).unwrap(),
            "\"question\""
        );
    }

    #[test]
    fn test_load_missing_fields_uses_defaults() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert!(config.clipboard_enabled);
        assert_eq!(config.mode, Mode::Echo);
        assert_eq!(config.translate_format, "{source}/{translated}");
        assert_eq!(config.question_format, "> {source}\nAI> {answer}");
    }

    #[test]
    fn test_steamvr_overlay_width_default_and_clamp() {
        let config: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(config.steamvr_overlay_width_m, 1.92);
        assert_eq!(clamp_overlay_width_m(2.0), 2.0);
        assert_eq!(clamp_overlay_width_m(0.0), 0.5);
        assert_eq!(clamp_overlay_width_m(100.0), 6.0);
        assert_eq!(clamp_overlay_width_m(f32::NAN), 1.92);
    }

    #[test]
    fn test_auto_input_and_vrchat_are_mutually_exclusive() {
        let mut config = Config::default();
        config.enable_vrchat_exclusive();
        assert!(config.vrchat_enabled);
        assert!(!config.auto_input_enabled);

        config.enable_auto_input_exclusive();
        assert!(config.auto_input_enabled);
        assert!(!config.vrchat_enabled);
    }

    #[test]
    fn test_translate_target_lang_presets() {
        let mut config = Config::default();
        assert_eq!(config.translate_target_lang(), "英語");

        config.translate_lang_preset = "CN".to_string();
        assert_eq!(config.translate_target_lang(), "中国語");

        config.translate_lang_preset = "CUSTOM".to_string();
        config.translate_lang_custom = "フランス語".to_string();
        assert_eq!(config.translate_target_lang(), "フランス語");
    }

    #[test]
    fn test_migrate_old_auto_translate_to_mode() {
        // 0.6 までの「自動翻訳 by Eliza」が ON だった設定は翻訳モードに引き継ぐ
        let json = r#"{"auto_translate_enabled": true, "translate_lang_preset": "CN"}"#;
        let config = Config::migrate_and_parse(json).unwrap();
        assert_eq!(config.mode, Mode::Translate);
        assert_eq!(config.translate_lang_preset, "CN");

        // OFF だった・キーが無い場合はエコー
        let config = Config::migrate_and_parse(r#"{"auto_translate_enabled": false}"#).unwrap();
        assert_eq!(config.mode, Mode::Echo);
        let config = Config::migrate_and_parse("{}").unwrap();
        assert_eq!(config.mode, Mode::Echo);

        // 新しい設定に mode があればそちらを優先
        let json = r#"{"auto_translate_enabled": true, "mode": "question"}"#;
        let config = Config::migrate_and_parse(json).unwrap();
        assert_eq!(config.mode, Mode::Question);
    }

    #[test]
    fn test_migrate_drops_eliza_keys() {
        // 0.6 までの Eliza の設定が残っていても読める。保存からは消える
        let json = r#"{"eliza_enabled": true, "eliza_url": "http://localhost:9096", "eliza_response_to_vrchat_enabled": true}"#;
        let config = Config::migrate_and_parse(json).unwrap();
        assert_eq!(config.mode, Mode::Echo);
        let saved = serde_json::to_string(&config).unwrap();
        assert!(!saved.contains("eliza"));
    }

    #[test]
    fn test_old_osc_mute_key_is_ignored() {
        // 0.5.0 の途中まであった設定(OSC でミュート2回を受ける)。残っていても読める
        let json = r#"{"vrchat_osc_mute_trigger_enabled": true, "vrchat_enabled": false}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(!config.vrchat_enabled);
        let saved = serde_json::to_string(&config).unwrap();
        assert!(!saved.contains("vrchat_osc_mute_trigger_enabled"));
    }
}
