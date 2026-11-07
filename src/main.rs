#![cfg_attr(windows, windows_subsystem = "windows")]

mod applog;
mod audio;
mod avatar_buttons;
mod config;
mod integrations;
mod steamvr;

slint::include_modules!();

use audio::AudioRecorder;
use audio::speech_to_text::SpeechToTextClient;
use avatar_buttons::{AvatarButton, ParamType, ToggleStates};
use config::{Config, Mode};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use integrations::{auto_input, grok, vrchat};
use slint::Model;
use std::cell::RefCell;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;
use steamvr::{OverlayAction, OverlayHandle, OverlaySnapshot};
use tokio::sync::mpsc::UnboundedReceiver;

fn main() -> Result<(), slint::PlatformError> {
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

    // Windows 11 標準の見た目に寄せる(ウィンドウ生成より前に決める)
    // SAFETY: まだスレッドを立てていない main の先頭でのみ呼ぶ
    unsafe { std::env::set_var("SLINT_STYLE", "fluent") };

    let config = Config::load();
    let window = MainWindow::new()?;
    let app = Rc::new(RefCell::new(App::new(config)));

    // 初期値を UI に流す
    {
        let app = app.borrow();
        app.sync_config_to_ui(&window);
        app.sync_display_to_ui(&window);
        let devices: Vec<slint::SharedString> = app
            .available_devices
            .iter()
            .cloned()
            .map(Into::into)
            .collect();
        window.set_devices(slint::ModelRc::new(slint::VecModel::from(devices)));
        window.set_device_index(app.selected_device_index as i32);
        window.set_silence_duration(app.config.silence_duration_secs);
        window.set_silence_threshold(app.config.silence_threshold);
    }

    wire_callbacks(&window, &app);

    // 録音の無音検知・チャネルの poll を回すタイマー。
    // VR の中でデスクトップ画面を見ていない間も状態を更新し続けるために常時回す
    // (egui 版の request_repaint_after(100ms) と同じ役割)
    let timer = slint::Timer::default();
    {
        let app = app.clone();
        let weak = window.as_weak();
        timer.start(
            slint::TimerMode::Repeated,
            std::time::Duration::from_millis(50),
            move || {
                let Some(window) = weak.upgrade() else { return };
                app.borrow_mut().tick(&window);
            },
        );
    }

    window.run()
}

/// 無音/発話の自動キャリブレーションの進行状態
struct Calibration {
    recorder: AudioRecorder,
    /// チャンク受信側。drop されると送信が詰まるので保持だけする
    _rx: tokio::sync::mpsc::UnboundedReceiver<Vec<f32>>,
    /// 0=無音(5秒), 1=発話(5秒), 2=無音(5秒)
    phase: u8,
    phase_start: Instant,
    noise_samples: Vec<f32>,
    speech_samples: Vec<f32>,
    /// フェーズ2終了時に計算した推奨 threshold
    new_threshold: Option<f32>,
    /// フェーズ3でレベルが threshold を下回るまでの秒数(発話の尾ひれ)
    tail_secs: Option<f32>,
}

const CALIB_PHASE_SECS: f32 = 5.0;

fn percentile(samples: &[f32], q: f32) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut v = samples.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = ((v.len() - 1) as f32 * q).round() as usize;
    v[idx]
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

    /// A/B/X/Y の今のオン/オフ(起動時はすべてオフ)
    avatar_states: ToggleStates,
    /// A/B/X/Y の設定の窓。開いている間だけ持つ
    avatar_window: Option<AvatarWindow>,

    available_devices: Vec<String>,
    selected_device_index: usize,

    /// オーバーレイ側から config が変わったら true。次の tick で UI に流す
    ui_config_dirty: bool,

    /// 録音開始/停止のグローバルショートカット
    hotkey_manager: Option<GlobalHotKeyManager>,
    registered_hotkey: Option<HotKey>,

    /// 実行中の自動キャリブレーション
    calibration: Option<Calibration>,
}

impl App {
    fn new(config: Config) -> Self {
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
        );

        let mut app = Self {
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
            avatar_states: ToggleStates::default(),
            avatar_window: None,
            available_devices,
            selected_device_index,
            ui_config_dirty: false,
            hotkey_manager: None,
            registered_hotkey: None,
            calibration: None,
        };
        app.register_shortcut();
        app
    }

    /// 自動キャリブレーションの開始/中止(設定ページのボタン)
    fn toggle_calibration(&mut self, window: &MainWindow) {
        if self.calibration.is_some() {
            self.calibration = None;
            window.set_calib_active(false);
            window.set_calib_guide("中止しました".into());
            window.set_level(0.0);
            return;
        }
        if self.is_recording {
            window.set_calib_guide("録音中はキャリブレーションできません".into());
            return;
        }
        let idx = window.get_device_index().max(0) as usize;
        let device_name = if idx == 0 {
            None
        } else {
            self.available_devices.get(idx).cloned()
        };
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut recorder = AudioRecorder::new(0.0);
        if let Err(e) = recorder.start_recording(device_name.as_deref(), tx) {
            window.set_calib_guide(format!("マイクを開けません: {}", e).into());
            return;
        }
        self.calibration = Some(Calibration {
            recorder,
            _rx: rx,
            phase: 0,
            phase_start: Instant::now(),
            noise_samples: Vec::new(),
            speech_samples: Vec::new(),
            new_threshold: None,
            tail_secs: None,
        });
        window.set_calib_active(true);
    }

    /// キャリブレーションの採取とフェーズ進行。tick から毎回呼ぶ
    fn tick_calibration(&mut self, window: &MainWindow) {
        let Some(calib) = &mut self.calibration else {
            return;
        };
        let level = calib.recorder.get_max_amplitude();
        let elapsed = calib.phase_start.elapsed().as_secs_f32();
        let remaining = (CALIB_PHASE_SECS - elapsed).max(0.0).ceil();
        match calib.phase {
            0 => {
                calib.noise_samples.push(level);
                window.set_calib_guide(
                    format!("① 無音のまま待ってください… あと {:.0} 秒", remaining).into(),
                );
            }
            1 => {
                calib.speech_samples.push(level);
                window
                    .set_calib_guide(format!("② 話してください… あと {:.0} 秒", remaining).into());
            }
            _ => {
                if calib.tail_secs.is_none()
                    && let Some(t) = calib.new_threshold
                    && level < t
                {
                    calib.tail_secs = Some(elapsed);
                }
                window.set_calib_guide(
                    format!("③ もう一度無音にしてください… あと {:.0} 秒", remaining).into(),
                );
            }
        }
        window.set_level(level);
        if let Some(t) = calib.new_threshold {
            window.set_level_below_threshold(level < t);
        }

        if elapsed < CALIB_PHASE_SECS {
            return;
        }
        match calib.phase {
            0 => {
                calib.phase = 1;
                calib.phase_start = Instant::now();
            }
            1 => {
                let noise = percentile(&calib.noise_samples, 0.95);
                let speech = percentile(&calib.speech_samples, 0.50);
                // 無音と発話の幾何平均を推奨 threshold とする
                let t = (noise * speech).sqrt().clamp(0.001, 0.3);
                calib.new_threshold = Some(t);
                calib.phase = 2;
                calib.phase_start = Instant::now();
            }
            _ => self.finish_calibration(window),
        }
    }

    /// 採取結果から推奨値をスライダーに反映する(config への確定は保存ボタン)
    fn finish_calibration(&mut self, window: &MainWindow) {
        let Some(calib) = self.calibration.take() else {
            return;
        };
        drop(calib.recorder);
        let threshold = calib.new_threshold.unwrap_or(self.config.silence_threshold);
        let duration = (calib.tail_secs.unwrap_or(0.5) + 0.5).clamp(0.5, 3.0);
        window.set_silence_threshold(threshold);
        window.set_silence_duration(duration);
        window.set_calib_active(false);
        window.set_level(0.0);
        let noise = percentile(&calib.noise_samples, 0.95);
        let speech = percentile(&calib.speech_samples, 0.50);
        if speech < noise * 1.5 {
            window.set_calib_guide(
                format!(
                    "声が小さいか検出できませんでした。暫定値 Threshold={:.3} / Duration={:.1}秒 を入れたのでスライダーで調整し、保存してください",
                    threshold, duration
                )
                .into(),
            );
        } else {
            window.set_calib_guide(
                format!(
                    "完了。推奨値 Threshold={:.3} / Duration={:.1}秒 を入れました。保存してください",
                    threshold, duration
                )
                .into(),
            );
        }
    }

    /// config.shortcut_key をグローバルショートカットとして登録し直す。空なら解除のみ
    fn register_shortcut(&mut self) {
        if let (Some(manager), Some(hotkey)) = (&self.hotkey_manager, &self.registered_hotkey) {
            let _ = manager.unregister(*hotkey);
        }
        self.registered_hotkey = None;
        let key = self.config.shortcut_key.trim();
        if key.is_empty() {
            return;
        }
        if self.hotkey_manager.is_none() {
            match GlobalHotKeyManager::new() {
                Ok(m) => self.hotkey_manager = Some(m),
                Err(e) => {
                    crate::log!("ホットキーマネージャの作成に失敗: {}", e);
                    return;
                }
            }
        }
        let manager = self.hotkey_manager.as_ref().unwrap();
        match HotKey::from_str(key) {
            Ok(hotkey) => match manager.register(hotkey) {
                Ok(()) => self.registered_hotkey = Some(hotkey),
                Err(e) => crate::log!("ショートカット {} の登録に失敗: {}", key, e),
            },
            Err(e) => crate::log!("ショートカット {} の解析に失敗: {}", key, e),
        }
    }

    /// 録音の開始/停止をトグルする(録音ボタンとショートカットの共通処理)
    fn toggle_recording(&mut self, window: &MainWindow) {
        if self.is_recording {
            self.is_recording = false;
            self.on_stop_recording();
        } else {
            self.is_recording = true;
            self.on_start_recording();
        }
        self.push_steamvr_snapshot();
        self.sync_display_to_ui(window);
    }

    /// 50ms ごとに呼ばれる。チャネルの回収・無音の自動停止・UI への反映をまとめてやる
    fn tick(&mut self, window: &MainWindow) {
        // グローバルショートカットのイベントを回収
        {
            let mut toggle = false;
            while let Ok(event) = GlobalHotKeyEvent::receiver().try_recv() {
                if event.state == HotKeyState::Pressed
                    && self.registered_hotkey.map(|h| h.id()) == Some(event.id)
                {
                    toggle = true;
                }
            }
            if toggle {
                crate::log!("ショートカットキーで録音をトグル");
                self.toggle_recording(window);
            }
        }

        self.tick_calibration(window);

        if let Some(overlay) = &self.steamvr_overlay
            && overlay.exit_requested()
        {
            self.exit_with_steamvr();
            return;
        }

        // Grok の加工結果を受け取る
        if let Some(ref receiver) = self.process_receiver
            && let Ok(result) = receiver.try_recv()
        {
            match result {
                Ok(output) => {
                    self.processed_output = output.clone();
                    self.dispatch_output(&output);
                    self.status_message = "完了".to_string();
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

        // オーバーレイからのアクション
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
            self.ui_config_dirty = true;
        }

        // 文字起こしの途中経過・結果
        if let Some(receiver) = &mut self.transcription_receiver
            && let Ok(message) = receiver.try_recv()
        {
            match message {
                TranscriptionMessage::Partial(text) => {
                    self.transcribed_text = text;
                    self.push_steamvr_snapshot();
                }
                TranscriptionMessage::Success(text) => {
                    self.transcribed_text = text;
                    self.on_transcription_success();
                }
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

        // 無音が続いたら録音停止
        if self.is_recording
            && let Some(recorder) = &self.audio_recorder
            && recorder.is_silent(self.config.silence_duration_secs)
        {
            self.is_recording = false;
            self.on_stop_recording();
            self.push_steamvr_snapshot();
        }

        if self.ui_config_dirty {
            self.sync_config_to_ui(window);
            self.ui_config_dirty = false;
        }
        self.sync_display_to_ui(window);
    }

    fn on_start_recording(&mut self) {
        self.status_message = "録音中... 話してください".to_string();

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
    /// ログを書き切ってから終了する
    fn exit_with_steamvr(&mut self) {
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
        let _ = slint::quit_event_loop();
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

    /// 加工が必要か
    fn needs_llm(&self) -> bool {
        self.config.proofread_enabled || self.config.mode != Mode::Echo
    }

    fn on_transcription_success(&mut self) {
        let text = self.transcribed_text.clone();
        self.status_message = "認識完了".to_string();
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
            // 翻訳・質問モードでは結果を待たず、まずソースを VRChat へ送る。
            // 結果が来たらフォーマット済みの文でもう一度送り直す(chatbox は上書き表示)
            if self.config.vrchat_enabled && mode != Mode::Echo {
                let pending_format = match mode {
                    Mode::Translate => &self.config.translate_pending_format,
                    _ => &self.config.question_pending_format,
                };
                let pending = grok::render_template(pending_format, &raw, "", "");
                if !pending.is_empty() {
                    let client = vrchat::VRChatClient::new();
                    if let Err(e) = client.send_message(&pending) {
                        crate::log!("VRChat OSC error (pending): {}", e);
                    }
                }
            }
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
                            let source = if proofread {
                                client.correct(&raw)?
                            } else {
                                raw
                            };
                            let answer = client.question(&source)?;
                            Ok(grok::render_template(
                                &question_format,
                                &source,
                                "",
                                &answer,
                            ))
                        }
                    }
                })();
                let _ = tx.send(result);
            });
            self.process_receiver = Some(rx);
            self.status_message = match self.config.mode {
                Mode::Echo => "校正中...".to_string(),
                Mode::Translate => "翻訳中...".to_string(),
                Mode::Question => "質問中...".to_string(),
            };
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

    /// config を保存し、VR に反映し、排他の結果を UI にも流し直す
    fn persist_and_push(&mut self, window: &MainWindow) {
        if let Err(e) = self.config.save() {
            crate::log!("Failed to save config: {}", e);
        }
        self.push_steamvr_snapshot();
        self.sync_config_to_ui(window);
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

    /// Config → UI(送り先と加工のコントロール)。起動時と、オーバーレイで変わったときに呼ぶ
    fn sync_config_to_ui(&self, window: &MainWindow) {
        window.set_clipboard_enabled(self.config.clipboard_enabled);
        window.set_auto_input_enabled(self.config.auto_input_enabled);
        window.set_send_enter_enabled(self.config.auto_input_send_enter);
        window.set_vrchat_enabled(self.config.vrchat_enabled);
        window.set_proofread_enabled(self.config.proofread_enabled);
        window.set_mode(match self.config.mode {
            Mode::Echo => 0,
            Mode::Translate => 1,
            Mode::Question => 2,
        });
        let langs: Vec<slint::SharedString> = self
            .config
            .translate_langs
            .iter()
            .cloned()
            .map(Into::into)
            .collect();
        window.set_lang_list(slint::ModelRc::new(slint::VecModel::from(langs)));
        window.set_lang_selected(self.config.translate_lang_selected.clone().into());
    }

    /// 状態 → UI(表示物)。毎 tick 呼ぶ
    fn sync_display_to_ui(&self, window: &MainWindow) {
        window.set_status_text(self.status_message.clone().into());
        window.set_transcribed_text(self.transcribed_text.clone().into());
        window.set_output_text(self.processed_output.clone().into());
        window.set_show_output(
            !self.processed_output.is_empty() && self.processed_output != self.transcribed_text,
        );
        window.set_is_recording(self.is_recording);
        window.set_steamvr_connected(
            self.steamvr_overlay
                .as_ref()
                .map(|o| o.is_connected())
                .unwrap_or(false),
        );

        // 警告(API キー未設定 / VR の異常)をまとめて出す
        let mut warnings: Vec<String> = Vec::new();
        if self.config.xai_api_key.is_empty() {
            warnings.push("⚠ xAI API key not set. Please configure in Settings.".to_string());
        }
        if let Some(problem) = self.steamvr_overlay.as_ref().and_then(|o| o.problem()) {
            warnings.push(format!("⚠ {}", problem));
        }
        window.set_warning_text(warnings.join("\n").into());

        // 録音の情報(時間・無音・音量)
        if self.is_recording
            && let Some(recorder) = &self.audio_recorder
        {
            window.set_recording_info(
                format!(
                    "録音: {:.1}秒 | 無音: {:.1}秒/{:.1}秒",
                    recorder.get_recording_duration(),
                    recorder.get_silence_duration().as_secs_f32(),
                    self.config.silence_duration_secs,
                )
                .into(),
            );
            let silence_progress = (recorder.get_silence_duration().as_secs_f32()
                / self.config.silence_duration_secs)
                .min(1.0);
            window.set_silence_progress(silence_progress);
            let level = recorder.get_max_amplitude();
            window.set_level(level);
            window.set_level_below_threshold(level < self.config.silence_threshold);
        } else if self.calibration.is_none() {
            window.set_recording_info("".into());
            window.set_silence_progress(0.0);
            window.set_level(0.0);
        }
    }

    /// A/B/X/Y の設定の窓を開く。保存できたら config に書いて VR にも反映する
    fn open_avatar_window(&mut self, app: Rc<RefCell<App>>) {
        if self.avatar_window.is_some() {
            return;
        }
        let avatar_window = match AvatarWindow::new() {
            Ok(w) => w,
            Err(e) => {
                crate::log!("Avatar window を開けない: {}", e);
                return;
            }
        };
        avatar_window.set_rows(rows_from_buttons(&self.config.avatar_buttons));

        {
            let app = app.clone();
            let weak = avatar_window.as_weak();
            avatar_window.on_save(move || {
                let Some(aw) = weak.upgrade() else { return };
                let mut app = app.borrow_mut();
                let rows = aw.get_rows();
                let mut new_buttons: [AvatarButton; 4] = app.config.avatar_buttons.clone();
                let mut errors: Vec<String> = Vec::new();
                for (i, row) in rows.iter().enumerate() {
                    new_buttons[i] = AvatarButton {
                        param: row.param.to_string(),
                        kind: match row.kind.as_str() {
                            "Int" => ParamType::Int,
                            "Float" => ParamType::Float,
                            _ => ParamType::Bool,
                        },
                        on: row.on.to_string(),
                        off: row.off.to_string(),
                    };
                    errors.push(new_buttons[i].error().unwrap_or_default());
                }
                if errors.iter().any(|e| !e.is_empty()) {
                    // エラーを表示して閉じない
                    let mut data: Vec<AvatarButtonData> = rows.iter().collect();
                    for (i, e) in errors.iter().enumerate() {
                        data[i].error = e.clone().into();
                    }
                    aw.set_rows(slint::ModelRc::new(slint::VecModel::from(data)));
                    aw.set_show_errors(true);
                    return;
                }
                let old_buttons = app.config.avatar_buttons.clone();
                app.avatar_states.reset_changed(&old_buttons, &new_buttons);
                app.config.avatar_buttons = new_buttons;
                if let Err(e) = app.config.save() {
                    crate::log!("Failed to save config: {}", e);
                }
                app.push_steamvr_snapshot();
                let _ = aw.hide();
                app.avatar_window = None;
            });
        }
        {
            let weak = avatar_window.as_weak();
            avatar_window.on_cancel(move || {
                let Some(aw) = weak.upgrade() else { return };
                let _ = aw.hide();
                app.borrow_mut().avatar_window = None;
            });
        }

        if let Err(e) = avatar_window.show() {
            crate::log!("Avatar window を表示できない: {}", e);
            return;
        }
        self.avatar_window = Some(avatar_window);
    }
}

/// ウィンドウの各ボタン・コントロールを App に結びつける
fn wire_callbacks(window: &MainWindow, app: &Rc<RefCell<App>>) {
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_start_stop(move || {
            let Some(window) = weak.upgrade() else { return };
            app.borrow_mut().toggle_recording(&window);
        });
    }

    // 送り先・加工の切り替えは即座に保存して VR にも反映する
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_toggle_clipboard(move |v| {
            let Some(window) = weak.upgrade() else { return };
            let mut app = app.borrow_mut();
            app.config.clipboard_enabled = v;
            app.persist_and_push(&window);
        });
    }
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_toggle_auto_input(move |v| {
            let Some(window) = weak.upgrade() else { return };
            let mut app = app.borrow_mut();
            if v {
                app.config.enable_auto_input_exclusive();
            } else {
                app.config.auto_input_enabled = false;
            }
            app.persist_and_push(&window);
        });
    }
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_toggle_send_enter(move |v| {
            let Some(window) = weak.upgrade() else { return };
            let mut app = app.borrow_mut();
            app.config.auto_input_send_enter = v;
            app.persist_and_push(&window);
        });
    }
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_toggle_vrchat(move |v| {
            let Some(window) = weak.upgrade() else { return };
            let mut app = app.borrow_mut();
            if v {
                app.config.enable_vrchat_exclusive();
            } else {
                app.config.vrchat_enabled = false;
            }
            app.persist_and_push(&window);
        });
    }
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_toggle_proofread(move |v| {
            let Some(window) = weak.upgrade() else { return };
            let mut app = app.borrow_mut();
            app.config.proofread_enabled = v;
            app.persist_and_push(&window);
        });
    }
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_set_mode(move |m| {
            let Some(window) = weak.upgrade() else { return };
            let mut app = app.borrow_mut();
            app.config.mode = match m {
                1 => Mode::Translate,
                2 => Mode::Question,
                _ => Mode::Echo,
            };
            app.persist_and_push(&window);
        });
    }
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_set_lang_selected(move |lang| {
            let Some(window) = weak.upgrade() else { return };
            let mut app = app.borrow_mut();
            app.config.translate_lang_selected = lang.to_string();
            app.persist_and_push(&window);
        });
    }
    {
        window.on_call_qvpen(move || {
            if let Err(e) = auto_input::call_qvpen() {
                crate::log!("call_qvpen error: {}", e);
            }
        });
    }

    // 設定ページ
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_start_calibration(move || {
            let Some(window) = weak.upgrade() else { return };
            app.borrow_mut().toggle_calibration(&window);
        });
    }
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_open_settings(move || {
            let Some(window) = weak.upgrade() else { return };
            let app = app.borrow();
            // 保存前の書きかけを捨てて config の今の値を出す
            window.set_settings_api_key(app.config.xai_api_key.clone().into());
            window.set_device_index(app.selected_device_index as i32);
            window.set_silence_duration(app.config.silence_duration_secs);
            window.set_silence_threshold(app.config.silence_threshold);
            window.set_translate_format(app.config.translate_format.clone().into());
            window.set_question_format(app.config.question_format.clone().into());
            window.set_translate_pending_format(app.config.translate_pending_format.clone().into());
            window.set_question_pending_format(app.config.question_pending_format.clone().into());
            window.set_translate_langs_text(app.config.translate_langs.join("/").into());
            window.set_shortcut_key(app.config.shortcut_key.clone().into());
            window.set_show_settings(true);
        });
    }
    {
        let app = app.clone();
        let weak = window.as_weak();
        window.on_save_settings(move || {
            let Some(window) = weak.upgrade() else { return };
            let mut app = app.borrow_mut();
            app.config.xai_api_key = window.get_settings_api_key().trim().to_string();
            let idx = window.get_device_index().max(0) as usize;
            app.selected_device_index = idx;
            app.config.input_device_name = if idx == 0 {
                None
            } else {
                app.available_devices.get(idx).cloned()
            };
            app.config.silence_duration_secs = window.get_silence_duration();
            app.config.silence_threshold = window.get_silence_threshold();
            app.config.translate_format = window.get_translate_format().to_string();
            app.config.question_format = window.get_question_format().to_string();
            app.config.translate_pending_format = window.get_translate_pending_format().to_string();
            app.config.question_pending_format = window.get_question_pending_format().to_string();
            app.config.translate_langs =
                crate::config::parse_translate_langs(&window.get_translate_langs_text());
            if !app
                .config
                .translate_langs
                .contains(&app.config.translate_lang_selected)
            {
                app.config.translate_lang_selected = app.config.translate_langs[0].clone();
            }
            app.config.shortcut_key = window.get_shortcut_key().trim().to_string();
            app.register_shortcut();
            if let Err(e) = app.config.save() {
                crate::log!("Failed to save config: {}", e);
            }
            app.push_steamvr_snapshot();
            app.sync_config_to_ui(&window);
            window.set_show_settings(false);
        });
    }
    {
        let weak = window.as_weak();
        window.on_cancel_settings(move || {
            if let Some(window) = weak.upgrade() {
                window.set_show_settings(false);
            }
        });
    }
    {
        let app = app.clone();
        window.on_open_avatar_settings(move || {
            let app_for_window = app.clone();
            app_for_window
                .borrow_mut()
                .open_avatar_window(app_for_window.clone());
        });
    }
    {
        let app = app.clone();
        window.on_open_bindings(move || {
            if let Some(overlay) = &app.borrow().steamvr_overlay {
                let _ = overlay
                    .command_tx
                    .send(steamvr::OverlayCommand::OpenBindingUi);
            }
        });
    }
}

/// Config の A/B/X/Y 設定を Slint のモデルに変換する
fn rows_from_buttons(buttons: &[AvatarButton; 4]) -> slint::ModelRc<AvatarButtonData> {
    let rows: Vec<AvatarButtonData> = buttons
        .iter()
        .enumerate()
        .map(|(i, b)| AvatarButtonData {
            label: avatar_buttons::LABELS[i].into(),
            param: b.param.clone().into(),
            kind: b.kind.label().into(),
            on: b.on.clone().into(),
            off: b.off.clone().into(),
            error: "".into(),
        })
        .collect();
    slint::ModelRc::new(slint::VecModel::from(rows))
}

#[cfg(test)]
mod shortcut_tests {
    use crate::percentile;
    use global_hotkey::hotkey::HotKey;
    use std::str::FromStr;

    #[test]
    fn test_shortcut_key_parse() {
        assert!(HotKey::from_str("F9").is_ok());
        assert!(HotKey::from_str("Ctrl+Shift+F9").is_ok());
        assert!(HotKey::from_str("ctrl+shift+r").is_ok());
        assert!(HotKey::from_str("not a key").is_err());
    }

    #[test]
    fn test_percentile() {
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[0.1, 0.2, 0.3], 0.0), 0.1);
        assert_eq!(percentile(&[0.1, 0.2, 0.3], 0.5), 0.2);
        assert_eq!(percentile(&[0.1, 0.2, 0.3], 0.95), 0.3);
        // 幾何平均の推奨値が無音と発話の間に入る
        let noise = percentile(&[0.01; 100], 0.95);
        let speech = percentile(&[0.3; 100], 0.50);
        let t = (noise * speech).sqrt().clamp(0.001, 0.3);
        assert!(noise < t && t < speech);
    }
}
