#![cfg_attr(windows, windows_subsystem = "windows")]

mod applog;
mod audio;
mod avatar_buttons;
mod config;
mod integrations;
mod steamvr;

use audio::AudioRecorder;
use audio::speech_to_text::SpeechToTextClient;
use avatar_buttons::{AvatarButton, LABELS, ParamType, ToggleStates};
use config::{Config, Mode};
use eframe::egui;
use integrations::{auto_input, grok, vrchat};
use std::sync::mpsc::{Receiver, channel};
use steamvr::{OverlayAction, OverlayHandle, OverlaySnapshot};
use tokio::sync::mpsc::UnboundedReceiver;

fn main() -> eframe::Result<()> {
    // 見た目の確認用: SteamVR なしでオーバーレイを BMP に書き出して終わる
    #[cfg(windows)]
    {
        let args: Vec<String> = std::env::args().collect();
        if let Some(i) = args.iter().position(|a| a == "--dump-overlay") {
            let prefix = args.get(i + 1).map(String::as_str).unwrap_or("overlay");
            if let Err(e) = steamvr::dump_overlay(prefix) {
                crate::log!("--dump-overlay 失敗: {}", e);
                std::process::exit(1);
            }
            return Ok(());
        }
    }
    applog::init();
    applog::install_panic_hook();
    crate::log!("[App] vrc-companion {} 起動", env!("CARGO_PKG_VERSION"));
    let config = Config::load();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([380.0, 680.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/logo.png"))
                    .expect("assets/logo.png"),
            ),
        ..Default::default()
    };

    eframe::run_native(
        "VRC Companion",
        options,
        Box::new(move |cc| {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "japanese".to_owned(),
                egui::FontData::from_static(include_bytes!("../fonts/NotoSansJP-Regular.ttf")),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "japanese".to_owned());
            cc.egui_ctx.set_fonts(fonts);

            Ok(Box::new(App::new(config, cc.egui_ctx.clone())))
        }),
    )
}

pub enum TranscriptionMessage {
    Partial(String),
    Success(String),
    Error(String),
}

struct App {
    config: Config,
    is_recording: bool,
    is_transcribing: bool,
    audio_recorder: Option<AudioRecorder>,
    transcribed_text: String,
    /// 加工後の出力(フォーマット済み。未加工なら認識結果そのまま)
    processed_output: String,
    status_message: String,

    transcription_receiver: Option<UnboundedReceiver<TranscriptionMessage>>,
    tokio_runtime: Option<tokio::runtime::Runtime>,
    /// Grok の加工(校正/翻訳/質問)の結果を受け取る
    process_receiver: Option<Receiver<Result<String, String>>>,
    steamvr_overlay: Option<OverlayHandle>,

    show_settings: bool,
    /// A/B/X/Y の今のオン/オフ(起動時はすべてオフ)
    avatar_states: ToggleStates,
    /// A/B/X/Y の設定の窓。開いている間の書きかけ
    avatar_draft: Option<[AvatarButton; 4]>,
    settings_xai_api_key: String,

    available_devices: Vec<String>,
    selected_device_index: usize,
}

impl App {
    fn new(config: Config, egui_ctx: egui::Context) -> Self {
        let mut available_devices = audio::get_input_devices().unwrap_or_else(|e| {
            crate::log!("Failed to get input devices: {}", e);
            vec![]
        });
        available_devices.insert(0, "デフォルト".to_string());

        let selected_device_index = config
            .input_device_name
            .as_ref()
            .and_then(|name| available_devices.iter().position(|d| d == name))
            .unwrap_or(0);

        let steamvr_overlay = steamvr::start(
            OverlaySnapshot::from_config(&config, false, "", "", &ToggleStates::default()),
            config.steamvr_overlay_width_m,
            egui_ctx,
        );

        Self {
            settings_xai_api_key: config.xai_api_key.clone(),
            config,
            is_recording: false,
            is_transcribing: false,
            audio_recorder: None,
            transcribed_text: String::new(),
            processed_output: String::new(),
            status_message: String::new(),
            transcription_receiver: None,
            tokio_runtime: None,
            process_receiver: None,
            steamvr_overlay,
            show_settings: false,
            avatar_states: ToggleStates::default(),
            avatar_draft: None,
            available_devices,
            selected_device_index,
        }
    }

    fn on_start_recording(&mut self) {
        self.status_message = "Recording... Speak now!".to_string();

        let mut recorder = AudioRecorder::new(self.config.silence_threshold);
        let (chunk_tx, chunk_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();
        let device_name = self.config.input_device_name.as_deref();

        match recorder.start_recording(device_name, chunk_tx) {
            Ok(_) => {
                let sample_rate = recorder.get_sample_rate();
                self.audio_recorder = Some(recorder);

                if self.config.xai_api_key.is_empty() {
                    self.status_message =
                        "Recording... (Set xAI API key in Settings to enable transcription)"
                            .to_string();
                } else {
                    self.start_streaming_transcription(sample_rate, chunk_rx);
                }
            }
            Err(e) => {
                self.status_message = format!("Error: {}", e);
                self.is_recording = false;
            }
        }
    }

    fn on_stop_recording(&mut self) {
        if let Some(mut recorder) = self.audio_recorder.take() {
            recorder.stop_recording();
        }
        self.status_message = if self.is_transcribing {
            "Transcribing...".to_string()
        } else {
            "Recording stopped.".to_string()
        };
    }

    /// SteamVR が終わってアプリごと閉じるとき。録音を止め(文字起こしの結果は待たない)、
    /// ログを書き切ってからウィンドウを閉じる
    fn exit_with_steamvr(&mut self, ctx: &egui::Context) {
        crate::log!("[App] SteamVR が終わったので終了する");
        if let Some(mut recorder) = self.audio_recorder.take() {
            recorder.stop_recording();
        }
        self.is_recording = false;
        self.transcription_receiver = None;
        if let Some(rt) = self.tokio_runtime.take() {
            // 文字起こしの通信の終わりを待たずに閉じる(UI スレッドを止めない)
            rt.shutdown_background();
        }
        use std::io::Write;
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn start_streaming_transcription(
        &mut self,
        sample_rate: u32,
        chunk_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<f32>>,
    ) {
        let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<TranscriptionMessage>();
        self.transcription_receiver = Some(msg_rx);
        self.is_transcribing = true;

        let api_key = self.config.xai_api_key.clone();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("tokio runtime");

        rt.spawn(async move {
            let client = SpeechToTextClient::new(api_key);
            if let Err(e) = client
                .stream_transcribe(sample_rate, chunk_rx, msg_tx.clone())
                .await
            {
                let _ = msg_tx.send(TranscriptionMessage::Error(e.to_string()));
            }
        });

        self.tokio_runtime = Some(rt);
    }

    /// 加工が必要か。質問モードに校正はかけない(質問のプロンプトが音声認識ノイズを前提にしているため)
    fn needs_llm(&self) -> bool {
        (self.config.proofread_enabled && self.config.mode != Mode::Question)
            || self.config.mode != Mode::Echo
    }

    fn on_transcription_success(&mut self, text: String) {
        self.transcribed_text = text.clone();
        self.status_message = "Transcription completed!".to_string();
        self.is_transcribing = false;
        self.transcription_receiver = None;
        if let Some(rt) = self.tokio_runtime.take() {
            rt.shutdown_background();
        }

        if text.is_empty() || !self.needs_llm() {
            self.processed_output = text.clone();
            self.dispatch_output(&text);
        } else if self.config.xai_api_key.is_empty() {
            crate::log!("xAI API key が無いので加工をスキップしてそのまま出力する");
            self.status_message = "⚠ xAI API key 未設定。加工せず出力しました".to_string();
            self.processed_output = text.clone();
            self.dispatch_output(&text);
        } else {
            let api_key = self.config.xai_api_key.clone();
            let mode = self.config.mode;
            let proofread = self.config.proofread_enabled;
            let target_lang = self.config.translate_target_lang();
            let translate_format = self.config.translate_format.clone();
            let question_format = self.config.question_format.clone();
            let raw = text;
            let (tx, rx) = channel::<Result<String, String>>();
            std::thread::spawn(move || {
                let result = (|| {
                    let client = grok::GrokClient::new(api_key);
                    match mode {
                        // 校正のみ
                        Mode::Echo => client.correct(&raw),
                        Mode::Translate => {
                            let source = if proofread {
                                client.correct(&raw)?
                            } else {
                                raw
                            };
                            let translated = client.translate(&target_lang, &source)?;
                            Ok(grok::render_template(
                                &translate_format,
                                &source,
                                &translated,
                                "",
                            ))
                        }
                        Mode::Question => {
                            let answer = client.question(&raw)?;
                            Ok(grok::render_template(&question_format, &raw, "", &answer))
                        }
                    }
                })();
                let _ = tx.send(result);
            });
            self.process_receiver = Some(rx);
            self.status_message = "Grok で処理中...".to_string();
        }
        self.push_steamvr_snapshot();
    }

    /// 出力先へ送る(クリップボード / アクティブウィンドウ / VRChat)
    fn dispatch_output(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.config.clipboard_enabled
            && let Ok(mut clipboard) = arboard::Clipboard::new()
        {
            let _ = clipboard.set_text(text);
        }
        if self.config.vrchat_enabled {
            let client = vrchat::VRChatClient::new();
            if let Err(e) = client.send_message(text) {
                crate::log!("VRChat OSC error: {}", e);
            }
        }
        if self.config.auto_input_enabled {
            let result = match (
                self.config.clipboard_enabled,
                self.config.auto_input_send_enter,
            ) {
                (true, true) => auto_input::send_ctrl_v_with_enter(),
                (true, false) => auto_input::send_ctrl_v(),
                (false, true) => auto_input::type_text_with_enter(text),
                (false, false) => auto_input::type_text(text),
            };
            if let Err(e) = result {
                crate::log!("Auto-input error: {}", e);
            }
        }
    }

    /// デスクトップ側の Config を VR オーバーレイ描画スレッドに反映する。
    fn push_steamvr_snapshot(&self) {
        if let Some(overlay) = &self.steamvr_overlay {
            let _ = overlay.snapshot_tx.send(OverlaySnapshot::from_config(
                &self.config,
                self.is_recording,
                &self.transcribed_text,
                &self.processed_output,
                &self.avatar_states,
            ));
        }
    }

    /// SteamVR が動いているときだけ「キー割り当て変更」(SteamVR の割り当て画面を PC の画面に開く)
    fn steamvr_bindings_button(&self, ui: &mut egui::Ui) {
        let Some(overlay) = self.steamvr_overlay.as_ref().filter(|o| o.is_connected()) else {
            return;
        };
        ui.add_space(10.0);
        ui.separator();
        if ui.button("キー割り当て変更").clicked() {
            let _ = overlay
                .command_tx
                .send(steamvr::OverlayCommand::OpenBindingUi);
        }
    }

    /// A/B/X/Y の設定の窓(別の窓)。保存すると config.json に書き、VR にも反映する
    fn avatar_settings_window(&mut self, ctx: &egui::Context) {
        if self.avatar_draft.is_none() {
            return;
        }
        let mut close = false;
        let mut save = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("avatar_buttons"),
            egui::ViewportBuilder::default()
                .with_title("VR のボタン A/B/X/Y")
                .with_inner_size([760.0, 560.0]),
            |ctx, _| {
                if ctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                let draft = self.avatar_draft.as_mut().expect("checked above");
                egui::CentralPanel::default()
                    .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(24.0))
                    .show(ctx, |ui| {
                        let all_ok = avatar_settings_ui(ui, draft);
                        ui.add_space(16.0);
                        ui.horizontal(|ui| {
                            let size = egui::vec2(120.0, 40.0);
                            if ui
                                .add_enabled(
                                    all_ok,
                                    egui::Button::new(egui::RichText::new("保存").size(18.0))
                                        .min_size(size),
                                )
                                .clicked()
                            {
                                save = true;
                            }
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new("キャンセル").size(18.0))
                                        .min_size(size),
                                )
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    });
            },
        );
        if save && let Some(new) = self.avatar_draft.take() {
            self.avatar_states
                .reset_changed(&self.config.avatar_buttons, &new);
            self.config.avatar_buttons = new;
            if let Err(e) = self.config.save() {
                crate::log!("Failed to save config: {}", e);
            }
            self.push_steamvr_snapshot();
        } else if close {
            self.avatar_draft = None;
        }
    }

    /// VR オーバーレイ側から受け取ったアクションを Config に適用する。
    fn apply_steamvr_action(&mut self, action: OverlayAction) {
        match action {
            OverlayAction::ToggleClipboard => {
                self.config.clipboard_enabled = !self.config.clipboard_enabled;
            }
            OverlayAction::ToggleAutoInput => {
                if self.config.auto_input_enabled {
                    self.config.auto_input_enabled = false;
                } else {
                    self.config.enable_auto_input_exclusive();
                }
            }
            OverlayAction::ToggleAutoInputSendEnter => {
                self.config.auto_input_send_enter = !self.config.auto_input_send_enter;
            }
            OverlayAction::ToggleVrchat => {
                if self.config.vrchat_enabled {
                    self.config.vrchat_enabled = false;
                } else {
                    self.config.enable_vrchat_exclusive();
                }
            }
            OverlayAction::ToggleProofread => {
                self.config.proofread_enabled = !self.config.proofread_enabled;
            }
            OverlayAction::SetMode(mode) => {
                self.config.mode = mode;
            }
            OverlayAction::CallQvPen => {
                if let Err(e) = auto_input::call_qvpen() {
                    crate::log!("call_qvpen error: {}", e);
                }
            }
            // オーバーレイのスレッドが自分で処理するので、ここには来ない
            OverlayAction::OpenBindingUi => {}
            OverlayAction::ToggleAvatarButton(i) => {
                if let Some((addr, value)) =
                    self.avatar_states.press(&self.config.avatar_buttons, i)
                {
                    crate::log!("[OSC] {} = {:?}", addr, value);
                    if let Err(e) = vrchat::VRChatClient::new().send_parameter(&addr, value) {
                        crate::log!("Failed to send avatar parameter: {}", e);
                    }
                }
            }
            OverlayAction::StartRecording => {
                // 録音中・文字起こし中は何もしない
                if !self.is_recording && !self.is_transcribing {
                    self.is_recording = true;
                    self.on_start_recording();
                }
            }
            OverlayAction::ToggleRecording => {
                if self.is_recording {
                    self.is_recording = false;
                    self.on_stop_recording();
                } else {
                    self.is_recording = true;
                    self.on_start_recording();
                }
            }
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // SteamVRオーバーレイのアクション(action_rx)はこのupdate()内でしかdrainされない。
        // 通常のeframeは反応的(次のOS入力/描画イベントまでupdate()を呼ばない)なので、
        // デスクトップウィンドウを見ていない間(VR内での操作中はまさにこの状態)は
        // オーバーレイでクリックしてもConfig反映・スナップショット再送が止まってしまい、
        // オーバーレイ側では古いスナップショットで再描画され続けてチェックが一瞬で戻る。
        // これを避けるため、SteamVR につながっている間は一定周期で強制的にrepaintさせる。
        // つながった・終了してほしい、はオーバーレイのスレッドが request_repaint で知らせてくる。
        if let Some(overlay) = &self.steamvr_overlay {
            if overlay.exit_requested() {
                self.exit_with_steamvr(ctx);
                return;
            }
            if overlay.is_connected() {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }

        // Grok の加工結果を受け取る。処理中は周期的に poll する
        if self.process_receiver.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(ref receiver) = self.process_receiver
            && let Ok(result) = receiver.try_recv()
        {
            match result {
                Ok(output) => {
                    self.processed_output = output.clone();
                    self.dispatch_output(&output);
                    self.status_message = "Done!".to_string();
                }
                Err(e) => {
                    crate::log!("Grok error: {}", e);
                    self.status_message = format!("Grok の処理に失敗: {}", e);
                    // 失敗したときは認識結果をそのまま出力する
                    let raw = self.transcribed_text.clone();
                    self.processed_output = raw.clone();
                    self.dispatch_output(&raw);
                }
            }
            self.process_receiver = None;
            self.push_steamvr_snapshot();
        }

        let steamvr_actions: Vec<OverlayAction> = self
            .steamvr_overlay
            .as_ref()
            .map(|overlay| overlay.action_rx.try_iter().collect())
            .unwrap_or_default();
        if !steamvr_actions.is_empty() {
            for action in steamvr_actions {
                self.apply_steamvr_action(action);
            }
            if let Err(e) = self.config.save() {
                crate::log!("Failed to save config: {}", e);
            }
            self.push_steamvr_snapshot();
        }

        if let Some(receiver) = &mut self.transcription_receiver
            && let Ok(message) = receiver.try_recv()
        {
            match message {
                TranscriptionMessage::Partial(text) => {
                    self.transcribed_text = text;
                    self.push_steamvr_snapshot();
                }
                TranscriptionMessage::Success(text) => self.on_transcription_success(text),
                TranscriptionMessage::Error(error) => {
                    self.status_message = format!("Transcription failed: {}", error);
                    self.is_transcribing = false;
                    self.transcription_receiver = None;
                    if let Some(rt) = self.tokio_runtime.take() {
                        rt.shutdown_background();
                    }
                }
            }
        }

        self.avatar_settings_window(ctx);

        if self.show_settings {
            egui::Window::new("Settings")
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("xAI API Key:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.settings_xai_api_key)
                            .password(true)
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(10.0);
                    ui.label("Input Device:");
                    egui::ComboBox::from_id_salt("input_device_combo")
                        .selected_text(
                            self.available_devices
                                .get(self.selected_device_index)
                                .cloned()
                                .unwrap_or_default(),
                        )
                        .show_ui(ui, |ui| {
                            for (idx, device_name) in self.available_devices.iter().enumerate() {
                                ui.selectable_value(
                                    &mut self.selected_device_index,
                                    idx,
                                    device_name,
                                );
                            }
                        });
                    ui.add_space(10.0);
                    ui.label("Silence Duration (seconds):");
                    ui.add(egui::Slider::new(
                        &mut self.config.silence_duration_secs,
                        0.5..=10.0,
                    ));
                    ui.label("Silence Threshold:");
                    ui.add(
                        egui::Slider::new(&mut self.config.silence_threshold, 0.001..=0.3)
                            .logarithmic(true),
                    );

                    ui.add_space(10.0);
                    ui.label("翻訳フォーマット ({source} {translated}):");
                    ui.text_edit_singleline(&mut self.config.translate_format);
                    ui.label("質問フォーマット ({source} {answer}):");
                    ui.text_edit_singleline(&mut self.config.question_format);

                    ui.add_space(10.0);
                    ui.separator();
                    if ui.button("VR のボタン A/B/X/Y の設定…").clicked() {
                        self.avatar_draft = Some(self.config.avatar_buttons.clone());
                    }

                    self.steamvr_bindings_button(ui);

                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            self.config.xai_api_key = self.settings_xai_api_key.trim().to_string();
                            self.config.input_device_name = if self.selected_device_index == 0 {
                                None
                            } else {
                                self.available_devices
                                    .get(self.selected_device_index)
                                    .cloned()
                            };
                            if let Err(e) = self.config.save() {
                                crate::log!("Failed to save config: {}", e);
                            }
                            self.push_steamvr_snapshot();
                            self.show_settings = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.settings_xai_api_key = self.config.xai_api_key.clone();
                            self.selected_device_index = self
                                .config
                                .input_device_name
                                .as_ref()
                                .and_then(|name| {
                                    self.available_devices.iter().position(|d| d == name)
                                })
                                .unwrap_or(0);
                            self.show_settings = false;
                        }
                    });
                });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("⚙ Settings").clicked() {
                            self.show_settings = true;
                        }
                    });
                });

                if self.config.xai_api_key.is_empty() {
                    ui.colored_label(
                        egui::Color32::RED,
                        "⚠ xAI API key not set. Please configure in Settings.",
                    );
                }
                if let Some(problem) = self.steamvr_overlay.as_ref().and_then(|o| o.problem()) {
                    ui.colored_label(
                        egui::Color32::from_rgb(0xe5, 0x48, 0x4d),
                        format!("⚠ {}", problem),
                    );
                }
                if !self.status_message.is_empty() {
                    ui.label(&self.status_message);
                }
                if self.is_recording
                    && let Some(recorder) = &self.audio_recorder
                {
                    ui.label(format!(
                        "Recording: {:.1}s | Silence: {:.1}s/{:.1}s",
                        recorder.get_recording_duration(),
                        recorder.get_silence_duration().as_secs_f32(),
                        self.config.silence_duration_secs,
                    ));
                }

                ui.add_space(10.0);

                let button_text = if self.is_recording {
                    "⏹ Stop"
                } else {
                    "⏺ Start"
                };
                let button_size = egui::vec2(300.0, 60.0);

                let silence_progress = if self.is_recording {
                    self.audio_recorder
                        .as_ref()
                        .map(|recorder| {
                            (recorder.get_silence_duration().as_secs_f32()
                                / self.config.silence_duration_secs)
                                .min(1.0)
                        })
                        .unwrap_or(0.0)
                } else {
                    0.0
                };

                let (rect, response) = ui.allocate_exact_size(button_size, egui::Sense::click());
                let visuals = ui.style().interact(&response);

                ui.painter()
                    .rect_filled(rect, visuals.rounding, visuals.bg_fill);

                if self.is_recording {
                    let fill_height = rect.height() * (1.0 - silence_progress);
                    if fill_height > 0.0 {
                        let progress_rect = egui::Rect::from_min_size(
                            egui::pos2(rect.min.x, rect.max.y - fill_height),
                            egui::vec2(rect.width(), fill_height),
                        );
                        ui.painter().rect_filled(
                            progress_rect,
                            visuals.rounding,
                            egui::Color32::from_rgb(100, 200, 255),
                        );
                    }
                }

                ui.painter()
                    .rect_stroke(rect, visuals.rounding, visuals.bg_stroke);
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    button_text,
                    egui::FontId::proportional(24.0),
                    visuals.text_color(),
                );

                if response.clicked() {
                    if self.is_recording {
                        self.is_recording = false;
                        self.on_stop_recording();
                    } else {
                        self.is_recording = true;
                        self.on_start_recording();
                    }
                    self.push_steamvr_snapshot();
                }

                ui.add_space(10.0);

                if self.is_recording {
                    if let Some(recorder) = &self.audio_recorder {
                        let max_amplitude = recorder.get_max_amplitude();
                        let bar_width = 300.0;
                        let bar_height = 10.0;
                        let clipped_amplitude = max_amplitude.min(1.2);
                        let bar_fill_width = (clipped_amplitude / 1.2) * bar_width;

                        let (bar_rect, _) = ui.allocate_exact_size(
                            egui::vec2(bar_width, bar_height),
                            egui::Sense::hover(),
                        );

                        ui.painter().rect_filled(
                            bar_rect,
                            2.0,
                            egui::Color32::from_rgb(50, 50, 50),
                        );

                        if bar_fill_width > 0.0 {
                            let fill_rect = egui::Rect::from_min_size(
                                bar_rect.min,
                                egui::vec2(bar_fill_width, bar_height),
                            );
                            let color = if max_amplitude < self.config.silence_threshold {
                                egui::Color32::from_rgb(150, 150, 150)
                            } else if max_amplitude < 1.0 {
                                egui::Color32::from_rgb(0, 200, 0)
                            } else {
                                egui::Color32::from_rgb(255, 0, 0)
                            };
                            ui.painter().rect_filled(fill_rect, 2.0, color);
                        }

                        ui.painter().rect_stroke(
                            bar_rect,
                            2.0,
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(100, 100, 100)),
                        );

                        ui.label(format!("Level: {:.3}", max_amplitude));
                    }

                    ui.add_space(10.0);
                }
                ui.label("Transcribed:");
                ui.add_sized(
                    [300.0, 60.0],
                    egui::TextEdit::multiline(&mut self.transcribed_text).interactive(false),
                );

                if !self.processed_output.is_empty()
                    && self.processed_output != self.transcribed_text
                {
                    ui.add_space(5.0);
                    ui.label("出力:");
                    ui.add_sized(
                        [300.0, 60.0],
                        egui::TextEdit::multiline(&mut self.processed_output).interactive(false),
                    );
                }

                ui.add_space(10.0);

                let mut changed = false;

                egui::TopBottomPanel::bottom("bottom_panel").show(ctx, |ui| {
                    egui::Frame::none()
                        .inner_margin(egui::Margin::symmetric(30.0, 0.0))
                        .show(ui, |ui| {
                            ui.vertical(|ui| {
                                ui.add_space(5.0);
                                ui.set_width(300.0);

                                changed |= ui
                                    .checkbox(
                                        &mut self.config.clipboard_enabled,
                                        "Copy to clipboard",
                                    )
                                    .changed();

                                let auto_input_changed = ui
                                    .checkbox(
                                        &mut self.config.auto_input_enabled,
                                        "Input to active window",
                                    )
                                    .changed();
                                if auto_input_changed && self.config.auto_input_enabled {
                                    self.config.enable_auto_input_exclusive();
                                }
                                changed |= auto_input_changed;

                                ui.indent("send_enter_indent", |ui| {
                                    changed |= ui
                                        .add_enabled(
                                            self.config.auto_input_enabled,
                                            egui::Checkbox::new(
                                                &mut self.config.auto_input_send_enter,
                                                "Send Enter after input",
                                            ),
                                        )
                                        .changed();
                                });

                                let vrchat_changed = ui
                                    .checkbox(&mut self.config.vrchat_enabled, "Input to VRChat")
                                    .changed();
                                if vrchat_changed && self.config.vrchat_enabled {
                                    self.config.enable_vrchat_exclusive();
                                }
                                changed |= vrchat_changed;

                                ui.separator();

                                changed |= ui
                                    .checkbox(&mut self.config.proofread_enabled, "校正 (Grok)")
                                    .changed();

                                ui.horizontal(|ui| {
                                    ui.label("モード:");
                                    for mode in Mode::ALL {
                                        if ui
                                            .selectable_label(
                                                self.config.mode == mode,
                                                mode.label(),
                                            )
                                            .clicked()
                                        {
                                            self.config.mode = mode;
                                            changed = true;
                                        }
                                    }
                                });

                                if self.config.mode == Mode::Translate {
                                    ui.indent("translate_lang_indent", |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label("翻訳先言語:");
                                            egui::ComboBox::from_id_salt("translate_lang_combo")
                                                .selected_text(
                                                    match self.config.translate_lang_preset.as_str()
                                                    {
                                                        "EN" => "EN",
                                                        "CN" => "CN",
                                                        _ => "自由記述",
                                                    },
                                                )
                                                .show_ui(ui, |ui| {
                                                    for (value, label) in [
                                                        ("EN", "EN"),
                                                        ("CN", "CN"),
                                                        ("CUSTOM", "自由記述"),
                                                    ] {
                                                        if ui
                                                            .selectable_value(
                                                                &mut self
                                                                    .config
                                                                    .translate_lang_preset,
                                                                value.to_string(),
                                                                label,
                                                            )
                                                            .changed()
                                                        {
                                                            changed = true;
                                                        }
                                                    }
                                                });
                                        });
                                        if self.config.translate_lang_preset == "CUSTOM" {
                                            changed |= ui
                                                .text_edit_singleline(
                                                    &mut self.config.translate_lang_custom,
                                                )
                                                .changed();
                                        }
                                    });
                                }

                                ui.add_space(5.0);
                                if ui.button("📝 call QvPen").clicked()
                                    && let Err(e) = auto_input::call_qvpen()
                                {
                                    crate::log!("call_qvpen error: {}", e);
                                }
                                ui.add_space(5.0);
                            })
                        })
                });

                if changed {
                    if let Err(e) = self.config.save() {
                        crate::log!("Failed to save config: {}", e);
                    }
                    self.push_steamvr_snapshot();
                }
            });
        });

        if self.is_recording {
            if let Some(recorder) = &self.audio_recorder
                && recorder.is_silent(self.config.silence_duration_secs)
            {
                self.is_recording = false;
                self.on_stop_recording();
                self.push_steamvr_snapshot();
            }
            ctx.request_repaint();
        }
        if self.is_transcribing {
            ctx.request_repaint();
        }
    }
}

/// A/B/X/Y の設定の中身。すべて正しければ true
fn avatar_settings_ui(ui: &mut egui::Ui, draft: &mut [AvatarButton; 4]) -> bool {
    ui.spacing_mut().item_spacing = egui::vec2(12.0, 10.0);
    ui.label(
        egui::RichText::new(
            "VR のパネル右端の丸いボタン。押すたびにオン/オフが切り替わり、\
             その値を VRChat のアバターパラメーターに送ります。名前が空のボタンは使いません。",
        )
        .size(15.0)
        .color(egui::Color32::from_gray(170)),
    );
    ui.add_space(8.0);
    let mut all_ok = true;
    for (i, b) in draft.iter_mut().enumerate() {
        egui::Frame::group(ui.style())
            .rounding(10.0)
            .inner_margin(egui::Margin::symmetric(16.0, 12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(LABELS[i]).size(28.0).strong());
                    ui.add_space(8.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut b.param)
                            .hint_text("パラメーター名（例: Hat）")
                            .font(egui::FontId::proportional(17.0))
                            .desired_width(200.0),
                    );
                    let before = b.kind;
                    egui::ComboBox::from_id_salt(("avatar_kind", i))
                        .selected_text(b.kind.label())
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for k in ParamType::ALL {
                                ui.selectable_value(&mut b.kind, k, k.label());
                            }
                        });
                    if b.kind != before {
                        let (on, off) = b.kind.default_values();
                        b.on = on.to_string();
                        b.off = off.to_string();
                    }
                    for (caption, value) in [("オン", &mut b.on), ("オフ", &mut b.off)] {
                        ui.label(egui::RichText::new(caption).size(15.0));
                        if b.kind == ParamType::Bool {
                            egui::ComboBox::from_id_salt(("avatar_bool", i, caption))
                                .selected_text(value.as_str())
                                .width(70.0)
                                .show_ui(ui, |ui| {
                                    for v in ["true", "false"] {
                                        ui.selectable_value(value, v.to_string(), v);
                                    }
                                });
                        } else {
                            ui.add(
                                egui::TextEdit::singleline(value)
                                    .font(egui::FontId::proportional(17.0))
                                    .desired_width(60.0),
                            );
                        }
                    }
                });
                if let Some(e) = b.error() {
                    all_ok = false;
                    ui.colored_label(egui::Color32::from_rgb(0xe5, 0x48, 0x4d), e);
                }
            });
    }
    all_ok
}
