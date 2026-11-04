//! `vrc-companion.exe --dump-overlay <出力先の接頭辞>`: SteamVR なしで、オーバーレイの見た目を
//! 本物の描画経路(WGL + egui_glow)で BMP に書き出す。見た目の確認・README 用。
//! サンプルの文字列は合成データ。

use super::bridge::{AvatarButtonView, OverlaySnapshot};
use super::overlay_ui::PIXELS_PER_POINT;
use super::render::GlOverlayRenderer;
use super::session::{OVERLAY_HEIGHT, OVERLAY_WIDTH};
use super::toast_ui::{self, ToastKind};

fn samples() -> Vec<(&'static str, OverlaySnapshot)> {
    let base = OverlaySnapshot {
        clipboard_enabled: true,
        auto_input_enabled: false,
        auto_input_send_enter: true,
        vrchat_enabled: true,
        proofread_enabled: true,
        mode: crate::config::Mode::Translate,
        is_recording: false,
        last_transcription: "こんにちは、今日はいい天気ですね。あとでワールド巡りしませんか？"
            .to_string(),
        last_output: "こんにちは、今日はいい天気ですね。あとでワールド巡りしませんか？/Hello, nice weather today. Want to go world hopping later?"
            .to_string(),
        // A オン、B・X オフ、Y 未設定
        avatar_buttons: std::array::from_fn(|i| AvatarButtonView {
            configured: i < 3,
            on: i == 0,
        }),
    };
    let recording = OverlaySnapshot {
        is_recording: true,
        mode: crate::config::Mode::Question,
        last_transcription: "おすすめのワールドある？".to_string(),
        last_output:
            "> おすすめのワールドある？\nAI> 星空がきれいな展望台のワールドはどうでしょう 🌙"
                .to_string(),
        ..base.clone()
    };
    vec![("idle", base), ("recording", recording)]
}

pub fn dump_overlay(prefix: &str) -> Result<(), String> {
    let mut renderer = GlOverlayRenderer::new(OVERLAY_WIDTH, OVERLAY_HEIGHT, PIXELS_PER_POINT)?;
    for (name, snapshot) in samples() {
        // 1 回目はフォントの準備とレイアウトの確定、2 回目を書き出す
        // スイッチのアニメーションが落ち着くまで数フレーム回す
        for _ in 0..30 {
            renderer.render(&snapshot, Vec::new())?;
        }
        let (w, h, rgba) = renderer.read_rgba()?;
        let path = format!("{}-{}.bmp", prefix, name);
        write_bmp(&path, w, h, &rgba).map_err(|e| format!("{}: {}", path, e))?;
        println!("{}", path);
    }

    // 録音の開始/終了トースト(背景は透明)
    let mut toast = GlOverlayRenderer::new(
        (toast_ui::TOAST_W * PIXELS_PER_POINT) as u32,
        (toast_ui::TOAST_H * PIXELS_PER_POINT) as u32,
        PIXELS_PER_POINT,
    )?;
    for (name, kind) in [
        ("toast-start", ToastKind::RecordingStarted),
        ("toast-stop", ToastKind::RecordingStopped),
    ] {
        for _ in 0..3 {
            toast.render_ui(Vec::new(), |ctx| toast_ui::draw(ctx, kind))?;
        }
        write_current(&mut toast, prefix, name)?;
    }

    Ok(())
}

fn write_current(renderer: &mut GlOverlayRenderer, prefix: &str, name: &str) -> Result<(), String> {
    let (w, h, rgba) = renderer.read_rgba()?;
    let path = format!("{}-{}.bmp", prefix, name);
    write_bmp(&path, w, h, &rgba).map_err(|e| format!("{}: {}", path, e))?;
    println!("{}", path);
    Ok(())
}

/// GL の読み出しは下の行から並ぶので、そのまま BMP(下から上)として書く。
fn write_bmp(path: &str, w: u32, h: u32, rgba: &[u8]) -> std::io::Result<()> {
    let data_len = (w * h * 4) as usize;
    let mut out = Vec::with_capacity(54 + data_len);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + data_len) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(data_len as u32).to_le_bytes());
    out.extend_from_slice(&[0u8; 16]);
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
    }
    std::fs::write(path, out)
}
