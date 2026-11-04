//! VR パネルの右端の A/B/X/Y ボタン。押すたびにオン/オフが切り替わり、
//! VRChat のアバターパラメーター `/avatar/parameters/<名前>` に OSC で値を送る。
//! 設定(名前・型・オンの値・オフの値)は PC の画面で決め、config.json に保存する。

use rosc::OscType;
use serde::{Deserialize, Serialize};

pub const LABELS: [&str; 4] = ["A", "B", "X", "Y"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ParamType {
    #[default]
    Bool,
    Int,
    Float,
}

impl ParamType {
    pub fn label(self) -> &'static str {
        match self {
            ParamType::Bool => "Bool",
            ParamType::Int => "Int (0〜255)",
            ParamType::Float => "Float (-1.0〜1.0)",
        }
    }
}

/// ボタン1つ分の設定。値は入力したままの文字列で持ち、使うときに確かめる。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvatarButton {
    /// 空なら「未設定」(VR では薄く表示され、押しても何もしない)
    #[serde(default)]
    pub param: String,
    #[serde(default)]
    pub kind: ParamType,
    #[serde(default = "default_on")]
    pub on: String,
    #[serde(default = "default_off")]
    pub off: String,
}

fn default_on() -> String {
    "true".to_string()
}

fn default_off() -> String {
    "false".to_string()
}

impl Default for AvatarButton {
    fn default() -> Self {
        Self {
            param: String::new(),
            kind: ParamType::Bool,
            on: default_on(),
            off: default_off(),
        }
    }
}

pub fn default_buttons() -> [AvatarButton; 4] {
    Default::default()
}

/// 1つの値を型に合わせて読み、範囲を確かめる
pub fn parse_value(kind: ParamType, s: &str) -> Result<OscType, String> {
    let s = s.trim();
    match kind {
        ParamType::Bool => match s.to_ascii_lowercase().as_str() {
            "true" => Ok(OscType::Bool(true)),
            "false" => Ok(OscType::Bool(false)),
            _ => Err("true か false".to_string()),
        },
        ParamType::Int => match s.parse::<i32>() {
            Ok(v) if (0..=255).contains(&v) => Ok(OscType::Int(v)),
            _ => Err("0〜255 の整数".to_string()),
        },
        ParamType::Float => match s.parse::<f32>() {
            Ok(v) if v.is_finite() && (-1.0..=1.0).contains(&v) => Ok(OscType::Float(v)),
            _ => Err("-1.0〜1.0 の数".to_string()),
        },
    }
}

const PREFIX: &str = "/avatar/parameters/";

/// 入力からパラメーター名だけを取り出す。先頭の / はあってもなくてもよく、
/// `/avatar/parameters/`(先頭の / なしも可)で始まっていればアドレスまるごととみなす
fn param_name(input: &str) -> &str {
    let s = input.strip_prefix('/').unwrap_or(input);
    s.strip_prefix(&PREFIX[1..]).unwrap_or(s)
}

/// 送るアドレス(必ず `/avatar/parameters/<名前>`)
pub fn address(input: &str) -> String {
    format!("{}{}", PREFIX, param_name(input.trim()))
}

/// パラメーター名を確かめる(空は「未設定」なので可)。
/// OSC のアドレスに使えない文字(空白と # * , ? [ ] { })、末尾の /、// は不可
pub fn check_param(input: &str) -> Result<(), String> {
    if input.is_empty() {
        return Ok(());
    }
    let name = param_name(input);
    if name.is_empty() {
        return Err("/avatar/parameters/ のあとに名前を書く".to_string());
    }
    if name.ends_with('/') {
        return Err("末尾に / は付けない".to_string());
    }
    if name.starts_with('/') || name.contains("//") {
        return Err("/ を2つ続けない".to_string());
    }
    if name
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || "#*,?[]{}".contains(c))
    {
        return Err("空白と # * , ? [ ] { } は使えない".to_string());
    }
    Ok(())
}

impl AvatarButton {
    pub fn is_configured(&self) -> bool {
        !self.param.trim().is_empty()
    }

    /// 入力のまちがい(なければ None)。未設定のボタンは値を見ない
    pub fn error(&self) -> Option<String> {
        let name = self.param.trim();
        if let Err(e) = check_param(name) {
            return Some(e);
        }
        if name.is_empty() {
            return None;
        }
        if let Err(e) = parse_value(self.kind, &self.on) {
            return Some(format!("オンの値: {}", e));
        }
        if let Err(e) = parse_value(self.kind, &self.off) {
            return Some(format!("オフの値: {}", e));
        }
        None
    }

    /// 状態 `on` のときに送る (アドレス, 値)。未設定・まちがいがあれば None
    pub fn message(&self, on: bool) -> Option<(String, OscType)> {
        if !self.is_configured() || self.error().is_some() {
            return None;
        }
        let v = parse_value(self.kind, if on { &self.on } else { &self.off }).ok()?;
        Some((address(&self.param), v))
    }
}

/// ボタンの今のオン/オフ(起動時はすべてオフ。保存しない)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToggleStates(pub [bool; 4]);

impl ToggleStates {
    /// ボタン `i` を押した。使えるボタンなら状態を切り替えて、送る (アドレス, 値) を返す
    pub fn press(&mut self, buttons: &[AvatarButton; 4], i: usize) -> Option<(String, OscType)> {
        let msg = buttons.get(i)?.message(!self.0[i])?;
        self.0[i] = !self.0[i];
        Some(msg)
    }

    /// 設定を変えたボタンはオフに戻す(前の名前の状態を引きずらない)
    pub fn reset_changed(&mut self, old: &[AvatarButton; 4], new: &[AvatarButton; 4]) {
        for i in 0..4 {
            if old[i] != new[i] {
                self.0[i] = false;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btn(param: &str, kind: ParamType, on: &str, off: &str) -> AvatarButton {
        AvatarButton {
            param: param.into(),
            kind,
            on: on.into(),
            off: off.into(),
        }
    }

    #[test]
    fn values_are_range_checked() {
        use ParamType::*;
        assert_eq!(parse_value(Bool, "true"), Ok(OscType::Bool(true)));
        assert_eq!(parse_value(Bool, " False "), Ok(OscType::Bool(false)));
        assert!(parse_value(Bool, "1").is_err());
        assert_eq!(parse_value(Int, "0"), Ok(OscType::Int(0)));
        assert_eq!(parse_value(Int, "255"), Ok(OscType::Int(255)));
        assert!(parse_value(Int, "256").is_err());
        assert!(parse_value(Int, "-1").is_err());
        assert!(parse_value(Int, "1.5").is_err());
        assert_eq!(parse_value(Float, "-1"), Ok(OscType::Float(-1.0)));
        assert_eq!(parse_value(Float, "0.5"), Ok(OscType::Float(0.5)));
        assert!(parse_value(Float, "1.01").is_err());
        assert!(parse_value(Float, "NaN").is_err());
        assert!(parse_value(Float, "").is_err());
    }

    #[test]
    fn address_accepts_name_leading_slash_or_full_address() {
        for input in [
            "Hat",
            "/Hat",
            "avatar/parameters/Hat",
            "/avatar/parameters/Hat",
            " /Hat ",
        ] {
            assert!(check_param(input.trim()).is_ok(), "{input:?}");
            assert_eq!(address(input), "/avatar/parameters/Hat", "{input:?}");
        }
        assert_eq!(address("Clothing/Hat"), "/avatar/parameters/Clothing/Hat");
        assert_eq!(
            address("/avatar/parameters/Clothing/Hat"),
            "/avatar/parameters/Clothing/Hat"
        );
        let b = btn("/avatar/parameters/Hat", ParamType::Bool, "true", "false");
        assert_eq!(
            b.message(true),
            Some(("/avatar/parameters/Hat".into(), OscType::Bool(true)))
        );
    }

    #[test]
    fn param_names_are_checked() {
        assert!(check_param("Clothing/Hat").is_ok());
        assert!(check_param("").is_ok());
        assert_eq!(check_param("Hat/"), Err("末尾に / は付けない".into()));
        assert_eq!(check_param("/Hat/"), Err("末尾に / は付けない".into()));
        assert_eq!(check_param("//Hat"), Err("/ を2つ続けない".into()));
        assert!(check_param("/avatar/parameters/").is_err());
        assert!(check_param("/avatar/parameters//Hat").is_err());
        for bad in ["a b", "Hat/", "a#b", "a*", "x?", "a\nb", "a//b", "/"] {
            assert!(check_param(bad).is_err(), "{bad:?}");
        }
        assert!(btn("Hat", ParamType::Int, "300", "0").error().is_some());
        assert!(btn("", ParamType::Int, "300", "0").error().is_none());
    }

    #[test]
    fn press_toggles_and_returns_the_message() {
        let mut b: [AvatarButton; 4] = default_buttons();
        b[1] = btn(" Hat ", ParamType::Int, "3", "0");
        let mut s = ToggleStates::default();
        assert_eq!(
            s.press(&b, 1),
            Some(("/avatar/parameters/Hat".into(), OscType::Int(3)))
        );
        assert_eq!(s.0, [false, true, false, false]);
        assert_eq!(
            s.press(&b, 1),
            Some(("/avatar/parameters/Hat".into(), OscType::Int(0)))
        );
        assert_eq!(s.0, [false; 4]);
    }

    #[test]
    fn unconfigured_or_invalid_buttons_do_nothing() {
        let mut b = default_buttons();
        b[2] = btn("Hat", ParamType::Float, "2.0", "0");
        let mut s = ToggleStates::default();
        assert_eq!(s.press(&b, 0), None);
        assert_eq!(s.press(&b, 2), None);
        assert_eq!(s.press(&b, 9), None);
        assert_eq!(s.0, [false; 4]);
    }

    #[test]
    fn changing_a_button_resets_only_that_state() {
        let old = [
            btn("A", ParamType::Bool, "true", "false"),
            btn("B", ParamType::Bool, "true", "false"),
            AvatarButton::default(),
            AvatarButton::default(),
        ];
        let mut new = old.clone();
        new[1].param = "B2".into();
        let mut s = ToggleStates([true, true, false, false]);
        s.reset_changed(&old, &new);
        assert_eq!(s.0, [true, false, false, false]);
    }

    #[test]
    fn config_json_roundtrip_and_missing_fields() {
        let b = btn("Hat", ParamType::Float, "0.5", "-1");
        let j = serde_json::to_string(&b).unwrap();
        assert_eq!(serde_json::from_str::<AvatarButton>(&j).unwrap(), b);
        let d: AvatarButton = serde_json::from_str("{}").unwrap();
        assert_eq!(d, AvatarButton::default());
    }
}
