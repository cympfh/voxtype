//! OpenVR ダッシュボードオーバーレイのライフサイクル管理。Windows専用 (`#[cfg(windows)]` は
//! 呼び出し元の bridge.rs 側でゲートしている)。
//!
//! `render::GlOverlayRenderer` が毎tick描画したegui UIのGLテクスチャを `SetOverlayTexture`
//! でSteamVRコンポジタに渡す。WGLコンテキストはスレッドアフィニティを持つため、
//! renderer の生成・描画・破棄は全てこのスレッド(`supervise`)の上で行う。
//!
//! SteamVR を起動してしまわないよう、vrserver.exe が動いているときだけ `VR_Init` する
//! (`runtime_watch`)。SteamVR が終わったら(VREvent_Quit、または vrserver.exe が居なくなったら)
//! OpenVR を呼ぶのをやめて切断し、また起動したらオーバーレイなどを作り直す。

use std::ffi::{CString, c_char, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use super::bridge::{OverlayAction, OverlayCommand, OverlayHandle, OverlaySnapshot};
use super::ffi::{self, OpenVrLibrary, Texture_t, VrOverlayHandle};
use super::input_manifest::{self, InputAction};
use super::manifest;
use super::overlay_ui::{LOGICAL_H, LOGICAL_W, PIXELS_PER_POINT};
use super::render::GlOverlayRenderer;
use super::runtime_watch::{self, AfterSession, RateLimited, SessionEnd, StatusLog};
use super::toast_ui::{self, ToastKind};

const OVERLAY_NAME: &str = "VRC Companion";
/// テクスチャの実ピクセル(論理サイズ × PIXELS_PER_POINT。以前は 400×380 を等倍で描いていた)
pub(super) const OVERLAY_WIDTH: u32 = (LOGICAL_W * PIXELS_PER_POINT) as u32;
pub(super) const OVERLAY_HEIGHT: u32 = (LOGICAL_H * PIXELS_PER_POINT) as u32;

pub fn start(initial: OverlaySnapshot, width_m: f32) -> Option<OverlayHandle> {
    let (snapshot_tx, snapshot_rx) = mpsc::channel::<OverlaySnapshot>();
    let (action_tx, action_rx) = mpsc::channel::<OverlayAction>();
    let (command_tx, command_rx) = mpsc::channel::<OverlayCommand>();
    let flags = SessionFlags {
        connected: Arc::new(AtomicBool::new(false)),
        exit_requested: Arc::new(AtomicBool::new(false)),
        problem: Arc::new(std::sync::Mutex::new(None)),
    };
    let connected = flags.connected.clone();
    let exit_requested = flags.exit_requested.clone();
    let problem = flags.problem.clone();

    let thread_problem = problem.clone();
    let inner_problem = problem.clone();
    let spawned = std::thread::Builder::new()
        .name("steamvr".into())
        .spawn(move || {
            // supervise 自体が落ちたら(run_session の外)つなぎ直せないので、画面にだけ出す
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                supervise(initial, width_m, snapshot_rx, action_tx, command_rx, flags)
            }));
            if r.is_err() {
                set_problem(
                    &inner_problem,
                    Some("VR の処理が止まった(アプリを再起動してください)".into()),
                );
            }
        });
    if let Err(e) = spawned {
        crate::log!("[SteamVR] スレッドを起動できない: {}", e);
        set_problem(&thread_problem, Some("VR の処理を起動できなかった".into()));
    }

    Some(OverlayHandle {
        snapshot_tx,
        action_rx,
        command_tx,
        connected,
        exit_requested,
        problem,
    })
}

/// 見張りのスレッドからアプリ(デスクトップ GUI)へ知らせるもの
struct SessionFlags {
    connected: Arc<AtomicBool>,
    exit_requested: Arc<AtomicBool>,
    problem: Arc<std::sync::Mutex<Option<String>>>,
}

fn set_problem(slot: &std::sync::Mutex<Option<String>>, value: Option<String>) {
    if let Ok(mut p) = slot.lock() {
        *p = value;
    }
}

/// VR の処理が落ちたら、この秒数待ってからつなぎ直す
const RESTART_AFTER_PANIC: Duration = Duration::from_secs(3);

impl SessionFlags {
    fn set_problem(&self, value: Option<String>) {
        set_problem(&self.problem, value);
    }

    fn set_connected(&self, on: bool) {
        self.connected.store(on, Ordering::SeqCst);
    }

    fn request_app_exit(&self) {
        self.exit_requested.store(true, Ordering::SeqCst);
    }
}

/// SteamVR が動いているときだけつなぐ(動いていなければ起動を待つ。SteamVR を起動することはない)。
/// つないだあとに SteamVR が終わっても切断して待機に戻り、起動したら自動でつなぎ直す。
/// アプリに終了を頼むのは、別コピーが既につないでいる(AnotherInstance)ときだけ。
fn supervise(
    initial: OverlaySnapshot,
    width_m: f32,
    snapshot_rx: Receiver<OverlaySnapshot>,
    action_tx: Sender<OverlayAction>,
    command_rx: Receiver<OverlayCommand>,
    flags: SessionFlags,
) {
    let mut latest = initial;
    let mut status = StatusLog::default();
    loop {
        // SteamVR が動いていないときに VR_Init すると SteamVR が起動してしまうので、先に確かめる
        if !runtime_watch::steamvr_running() {
            status.log("SteamVR の起動を待っています(起動したら自動でつなぐ)");
        } else {
            match OpenVrLibrary::load() {
                Err(e) => status.log(&format!(
                    "つなげなかった({}秒後にやり直す): {}",
                    runtime_watch::RETRY_EVERY.as_secs(),
                    e
                )),
                Ok(lib) => {
                    status.log("SteamVR につないだ");
                    flags.set_problem(None);
                    flags.set_connected(true);
                    // 中でパニックしてもスレッドごと死なず、つなぎ直す(前はオーバーレイが黙って消えた)
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        run_session(
                            &lib,
                            width_m,
                            &mut latest,
                            &snapshot_rx,
                            &action_tx,
                            &command_rx,
                        )
                    }));
                    flags.set_connected(false);
                    let end = match result {
                        Ok(end) => end,
                        Err(payload) => {
                            drop(lib);
                            crate::log!(
                                "[SteamVR] VR の処理が落ちた({}秒後につなぎ直す): {}",
                                RESTART_AFTER_PANIC.as_secs(),
                                crate::applog::payload_message(payload.as_ref())
                            );
                            flags.set_problem(Some(
                                "VR の処理が落ちたので、つなぎ直しています".into(),
                            ));
                            status = StatusLog::default();
                            std::thread::sleep(RESTART_AFTER_PANIC);
                            continue;
                        }
                    };
                    // オーバーレイ・レンダラ・入力は run_session の中で片付いている。ここで VR_Shutdown
                    // (アプリに終了を頼むのは切断が済んでから)
                    drop(lib);
                    status.log(&end.message());
                    match end.after() {
                        AfterSession::ExitApp => {
                            flags.request_app_exit();
                            return;
                        }
                        AfterSession::ThreadDone => return,
                        AfterSession::Retry => {}
                    }
                }
            }
        }
        if !wait_while_disconnected(&snapshot_rx, &command_rx, &mut latest) {
            return;
        }
    }
}

/// つながっていない間、`RETRY_EVERY` だけ待つ。その間もアプリからのスナップショットは受け取って
/// 最新を保ち、設定画面の頼みごとは捨てる。アプリが閉じたら false
fn wait_while_disconnected(
    snapshot_rx: &Receiver<OverlaySnapshot>,
    command_rx: &Receiver<OverlayCommand>,
    latest: &mut OverlaySnapshot,
) -> bool {
    let until = Instant::now() + runtime_watch::RETRY_EVERY;
    loop {
        let now = Instant::now();
        if now >= until {
            return true;
        }
        match snapshot_rx.recv_timeout(until - now) {
            Ok(s) => *latest = s,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return false,
        }
        while command_rx.try_recv().is_ok() {}
    }
}

/// SteamVR が終了を知らせてきたか(IVRSystem のイベントを空になるまで読む)
fn quit_requested(lib: &OpenVrLibrary) -> bool {
    let Some(system) = lib.system() else {
        return false;
    };
    let mut quit = false;
    loop {
        let mut event: ffi::VREvent_t = unsafe { std::mem::zeroed() };
        let has = unsafe {
            (system.poll_next_event)(&mut event, std::mem::size_of::<ffi::VREvent_t>() as u32)
        };
        if !has {
            break;
        }
        if event.event_type == ffi::EVENT_QUIT {
            quit = true;
        }
    }
    quit
}

/// SteamVR につながっている間の1回分。オーバーレイ(タブとトースト)・入力・自動起動の登録を
/// すべてここで作り、抜けるときにレンダラなどは drop される(オーバーレイ自体は VR_Shutdown で消える)
fn run_session(
    lib: &OpenVrLibrary,
    width_m: f32,
    latest: &mut OverlaySnapshot,
    snapshot_rx: &Receiver<OverlaySnapshot>,
    action_tx: &Sender<OverlayAction>,
    command_rx: &Receiver<OverlayCommand>,
) -> SessionEnd {
    // SteamVR Input(ボタン割り当て)用のファイルを先に書く。vrmanifest に場所を書き込むため
    let action_manifest = match input_manifest::default_input_dir()
        .and_then(|dir| input_manifest::write_input_files(&dir))
    {
        Ok(path) => Some(path),
        Err(e) => {
            crate::log!(
                "[SteamVR] アクションマニフェスト書き込み失敗 (ボタン割り当ては無効): {}",
                e
            );
            None
        }
    };

    ensure_auto_launch_registered(lib, action_manifest.as_deref());

    // SetActionManifestPath は最初の PollNextEvent より前に呼ぶ必要がある(openvr.h)ので、
    // オーバーレイのイベントを読み始める前にここで初期化する
    let mut input = action_manifest
        .as_deref()
        .and_then(|path| SteamVrInput::init(lib, path));

    let overlay_handle = match create_dashboard_overlay(lib) {
        Ok(h) => h,
        Err(DashboardError::KeyInUse) => return SessionEnd::AnotherInstance,
        Err(DashboardError::Other(e)) => {
            return SessionEnd::Failed(format!("CreateDashboardOverlay: {}", e));
        }
    };

    if let Err(e) = show_overlay(lib, overlay_handle) {
        crate::log!("[SteamVR] ShowOverlay 失敗: {}", e);
    }

    // SetOverlayInputMethod を呼ばない限りコンポジタはこのオーバーレイに対して
    // MouseMove/MouseButtonDown/Up を一切生成しない(デフォルトはInputMethod_None)。
    // これが無いと表示だけされてレーザーポインタ操作が効かない。
    if let Err(e) = set_overlay_input_method(lib, overlay_handle) {
        crate::log!("[SteamVR] SetOverlayInputMethod 失敗: {}", e);
    }
    // マウス座標は UI の論理ピクセルで受け取る(テクスチャは PIXELS_PER_POINT 倍の解像度)
    if let Err(e) = set_overlay_mouse_scale(lib, overlay_handle, LOGICAL_W, LOGICAL_H) {
        crate::log!("[SteamVR] SetOverlayMouseScale 失敗: {}", e);
    }
    // 表示の大きさ(幅)。呼ばないと SteamVR の既定の 1m になり、小さくて読みにくかった
    if let Err(e) = set_overlay_width_in_meters(lib, overlay_handle, width_m) {
        crate::log!("[SteamVR] SetOverlayWidthInMeters 失敗: {}", e);
    }

    let mut renderer = match GlOverlayRenderer::new(OVERLAY_WIDTH, OVERLAY_HEIGHT, PIXELS_PER_POINT)
    {
        Ok(r) => r,
        Err(e) => return SessionEnd::Failed(format!("GlOverlayRenderer: {}", e)),
    };

    // 録音の開始/終了のトースト(頭に固定した別オーバーレイ)。作れなくても本体は動かす
    let mut toast = match Toast::create(lib) {
        Ok(t) => Some(t),
        Err(e) => {
            crate::log!("[SteamVR] トースト用オーバーレイを作れなかった: {}", e);
            None
        }
    };

    let mut was_recording = latest.is_recording;
    let mut last_alive_check = Instant::now();
    let mut frame_sync_errors = RateLimited::new(Duration::from_secs(60));
    let mut texture_errors = RateLimited::new(Duration::from_secs(60));
    loop {
        // WaitFrameSync でコンポジタの次フレームに同期する(固定インターバルの
        // recv_timeoutによるポーリングではなく、コンポジタ側のペースに揃える)。
        // タイムアウトしても(戻り値がエラーでも)描画自体は続ける — SteamVRが
        // 一時的にフレームを配れない状況(例: ダッシュボード非表示中)でも
        // オーバーレイの更新を止めたくないため。
        if let Err(e) = wait_frame_sync(lib, 100)
            && let Some(line) = frame_sync_errors.check(Instant::now(), &e)
        {
            crate::log!("[SteamVR] WaitFrameSync 失敗: {}", line);
        }

        // SteamVR が終わるなら、ほかの OpenVR 呼び出しより先に気づいて抜ける
        if quit_requested(lib) {
            // 終了の知らせを受け取ったと伝え、すぐ VR_Shutdown する(呼び出し元で lib を drop)。
            // プロセスは終わらなくてよい(SteamVR が気にするのは vrclient がつながったままかどうか)
            if let Some(system) = lib.system() {
                unsafe { (system.acknowledge_quit_exiting)() };
            }
            return SessionEnd::SteamVrQuit;
        }
        if last_alive_check.elapsed() >= runtime_watch::ALIVE_CHECK_EVERY {
            last_alive_check = Instant::now();
            if !runtime_watch::steamvr_running() {
                return SessionEnd::SteamVrGone;
            }
        }

        match snapshot_rx.try_recv() {
            Ok(s) => *latest = s,
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => return SessionEnd::AppClosed,
        }

        if let Some(t) = toast.as_mut() {
            if let Some(kind) = ToastKind::from_transition(was_recording, latest.is_recording) {
                t.show(lib, kind);
            }
            t.tick(lib);
        }
        was_recording = latest.is_recording;

        // SteamVR のバインドで割り当てられたボタン操作(VRChat を操作中でも届く)
        if let Some(input) = input.as_mut() {
            for action in input.poll(lib) {
                let _ = action_tx.send(OverlayAction::from_input(action));
            }
        }

        // デスクトップの設定画面からの頼みごと
        while let Ok(cmd) = command_rx.try_recv() {
            match cmd {
                OverlayCommand::OpenBindingUi => match input.as_ref() {
                    Some(i) => i.open_binding_ui(lib, true),
                    None => {
                        crate::log!("[SteamVR] SteamVR Input が使えないので割り当て画面を開けない")
                    }
                },
            }
        }

        let events = poll_overlay_events(lib, overlay_handle);
        match renderer.render(latest, events) {
            Ok((texture, actions)) => {
                if !actions.is_empty() {
                    if let Err(e) = trigger_laser_mouse_haptic_vibration(lib, overlay_handle) {
                        crate::log!("[SteamVR] TriggerLaserMouseHapticVibration 失敗: {}", e);
                    }
                }
                for action in actions {
                    if action == OverlayAction::OpenBindingUi {
                        // VR の中(ダッシュボード)で SteamVR の割り当て画面を開く
                        match input.as_ref() {
                            Some(i) => i.open_binding_ui(lib, false),
                            None => crate::log!(
                                "[SteamVR] SteamVR Input が使えないので割り当て画面を開けない"
                            ),
                        }
                        continue;
                    }
                    let _ = action_tx.send(action);
                }
                let mut tex = Texture_t {
                    handle: texture.0.get() as usize as *mut c_void,
                    e_type: ffi::TEXTURE_TYPE_OPENGL,
                    e_color_space: ffi::COLOR_SPACE_AUTO,
                };
                if let Err(e) = set_overlay_texture(lib, overlay_handle, &mut tex) {
                    // ハンドルが無効 = サーバ側のオーバーレイが消えた(SteamVR 再起動など)。
                    // このまま回り続けても直らないので、セッションを畳んでつなぎ直す
                    if e == ffi::OVERLAY_ERROR_INVALID_HANDLE {
                        return SessionEnd::Failed("SetOverlayTexture: ハンドルが無効".to_string());
                    }
                    if let Some(line) =
                        texture_errors.check(Instant::now(), &format!("EVROverlayError={}", e))
                    {
                        crate::log!("[SteamVR] SetOverlayTexture 失敗: {}", line);
                    }
                }
            }
            Err(e) => crate::log!("[SteamVR] render 失敗: {}", e),
        }
    }
}

/// `PollNextOverlayEvent` を空になるまでポーリングし、マウス関連イベントを
/// `egui::Event` に変換する。
///
/// - Y座標: 実機検証の結果、OpenVRのオーバーレイマウス座標は反転不要(既にegui同様の
///   原点左上・Y下向き)だったため、`mouse.y`をそのまま使う(以前あった`height - mouse.y`
///   の反転は誤りだった)。
/// - ボタン: ダッシュボードのレーザーポインタ操作は実質「トリガー押下=クリック」の
///   単一ボタン操作であり、UI側(`render.rs`)も`Response::clicked()`(Primaryボタン限定)
///   しか見ていない。`VREvent_Mouse_t.button`のビット値解釈に依存すると、離す側の
///   イベントでこのフィールドが押す側と一致しない/正しく載らない実装がある場合に
///   クリックとして成立しなくなるため、ボタン種別に関わらず常にPrimaryとして扱う。
fn poll_overlay_events(lib: &OpenVrLibrary, handle: VrOverlayHandle) -> Vec<egui::Event> {
    let mut events = Vec::new();
    loop {
        let mut vr_event: ffi::VREvent_t = unsafe { std::mem::zeroed() };
        let has_event = unsafe {
            (lib.overlay().poll_next_overlay_event)(
                handle,
                &mut vr_event,
                std::mem::size_of::<ffi::VREvent_t>() as u32,
            )
        };
        if !has_event {
            break;
        }

        if matches!(
            vr_event.event_type,
            ffi::EVENT_MOUSE_MOVE | ffi::EVENT_MOUSE_BUTTON_DOWN | ffi::EVENT_MOUSE_BUTTON_UP
        ) {
            let mouse = unsafe { vr_event.data.mouse };
            let pos = egui::pos2(mouse.x, mouse.y);
            events.push(egui::Event::PointerMoved(pos));

            match vr_event.event_type {
                ffi::EVENT_MOUSE_BUTTON_DOWN => events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::default(),
                }),
                ffi::EVENT_MOUSE_BUTTON_UP => events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::default(),
                }),
                _ => {}
            }
        }
    }
    events
}

/// SteamVR起動時にこのexeを自動起動してもらうよう、アプリケーションマニフェストを登録する。
/// IVRApplicationsインターフェース取得失敗・マニフェスト書き込み失敗はいずれも非致命的に扱い、
/// オーバーレイ本体の起動をブロックしない(既存のSetOverlayInputMethod等と同じパターン)。
fn ensure_auto_launch_registered(lib: &OpenVrLibrary, action_manifest: Option<&std::path::Path>) {
    let Some(applications) = lib.applications() else {
        return;
    };

    let path = match manifest::write_manifest(action_manifest) {
        Ok(path) => path,
        Err(e) => {
            crate::log!("[SteamVR] マニフェスト書き込み失敗: {}", e);
            return;
        }
    };

    let path_c = match CString::new(path.to_string_lossy().into_owned()) {
        Ok(s) => s,
        Err(e) => {
            crate::log!("[SteamVR] マニフェストパスのCString化失敗: {}", e);
            return;
        }
    };
    let err =
        unsafe { (applications.add_application_manifest)(path_c.as_ptr() as *mut c_char, false) };
    if err != 0 {
        crate::log!(
            "[SteamVR] AddApplicationManifest 失敗: EVRApplicationError={}",
            err
        );
        return;
    }

    let key = CString::new(manifest::APP_KEY).unwrap();

    // 手で起動した(SteamVR の自動起動でない)場合、SteamVR はこのプロセスがどのアプリか
    // 分からず、ボタン割り当てが別アプリ扱いになる。自分で名乗っておく。
    // ヘッダには「0 = 自分自身」とあるが、実機の vrserver ログでは 0 を渡すとアプリの PID が
    // 0 にされたので、自分の PID を明示する。
    // SteamVR から起動された場合はすでに分かっているのでエラーが返ることがあるが、問題ない
    let err = unsafe {
        (applications.identify_application)(std::process::id(), key.as_ptr() as *mut c_char)
    };
    if err != 0 {
        crate::log!(
            "[SteamVR] IdentifyApplication: EVRApplicationError={} (SteamVR から起動された場合は無視してよい)",
            err
        );
    }

    let err =
        unsafe { (applications.set_application_auto_launch)(key.as_ptr() as *mut c_char, true) };
    if err != 0 {
        crate::log!(
            "[SteamVR] SetApplicationAutoLaunch 失敗: EVRApplicationError={}",
            err
        );
    } else {
        crate::log!("[SteamVR] SteamVR の自動起動に登録した: {}", path.display());
    }
}

/// 録音の開始/終了を知らせるトースト。ダッシュボードとは別の普通のオーバーレイで、
/// ヘッドセットに固定(目の前の少し下)なので、ダッシュボードを閉じていても見える。
struct Toast {
    handle: VrOverlayHandle,
    renderer: GlOverlayRenderer,
    shown_at: Option<Instant>,
}

impl Toast {
    fn create(lib: &OpenVrLibrary) -> Result<Self, String> {
        // 先に描画の準備(失敗したらオーバーレイは作らない)
        let renderer = GlOverlayRenderer::new(
            (toast_ui::TOAST_W * PIXELS_PER_POINT) as u32,
            (toast_ui::TOAST_H * PIXELS_PER_POINT) as u32,
            PIXELS_PER_POINT,
        )?;
        let key = CString::new(format!("{}.toast", manifest::APP_KEY)).unwrap();
        let name = CString::new("VRC Companion 通知").unwrap();
        let mut handle: VrOverlayHandle = 0;
        let ov = lib.overlay();
        let err = unsafe {
            (ov.create_overlay)(
                key.as_ptr() as *mut c_char,
                name.as_ptr() as *mut c_char,
                &mut handle,
            )
        };
        if err != 0 {
            return Err(format!("CreateOverlay: EVROverlayError={}", err));
        }
        let [x, y, z] = toast_ui::TOAST_OFFSET_M;
        let mut transform = ffi::HmdMatrix34_t {
            m: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, y], [0.0, 0.0, 1.0, z]],
        };
        unsafe {
            let e = (ov.set_overlay_width_in_meters)(handle, toast_ui::TOAST_WIDTH_M);
            if e != 0 {
                crate::log!(
                    "[SteamVR] トースト SetOverlayWidthInMeters: EVROverlayError={}",
                    e
                );
            }
            let e = (ov.set_overlay_transform_tracked_device_relative)(
                handle,
                ffi::TRACKED_DEVICE_INDEX_HMD,
                &mut transform,
            );
            if e != 0 {
                crate::log!(
                    "[SteamVR] トースト SetOverlayTransformTrackedDeviceRelative: EVROverlayError={}",
                    e
                );
            }
            // 他のオーバーレイより手前に
            let _ = (ov.set_overlay_sort_order)(handle, 100);
            let _ = (ov.hide_overlay)(handle);
        }
        Ok(Self {
            handle,
            renderer,
            shown_at: None,
        })
    }

    fn show(&mut self, lib: &OpenVrLibrary, kind: ToastKind) {
        // テクスチャを渡すのは描いた直後(このレンダラの GL コンテキストが有効な間)
        let texture = match self
            .renderer
            .render_ui(Vec::new(), |ctx| toast_ui::draw(ctx, kind))
        {
            Ok(t) => t,
            Err(e) => {
                crate::log!("[SteamVR] トーストの描画に失敗: {}", e);
                return;
            }
        };
        let mut tex = Texture_t {
            handle: texture.0.get() as usize as *mut c_void,
            e_type: ffi::TEXTURE_TYPE_OPENGL,
            e_color_space: ffi::COLOR_SPACE_AUTO,
        };
        if let Err(e) = set_overlay_texture(lib, self.handle, &mut tex) {
            crate::log!("[SteamVR] トースト SetOverlayTexture 失敗: {}", e);
            return;
        }
        unsafe {
            let _ = (lib.overlay().set_overlay_alpha)(self.handle, 0.0);
        }
        if let Err(e) = show_overlay(lib, self.handle) {
            crate::log!("[SteamVR] トースト ShowOverlay 失敗: {}", e);
            return;
        }
        self.shown_at = Some(Instant::now());
        crate::log!("[SteamVR] トースト: {:?}", kind);
    }

    /// 毎フレーム呼ぶ。透明度を時間に合わせて変え、時間が来たら隠す
    fn tick(&mut self, lib: &OpenVrLibrary) {
        let Some(at) = self.shown_at else {
            return;
        };
        let ov = lib.overlay();
        match toast_ui::alpha_at(at.elapsed()) {
            Some(a) => unsafe {
                let _ = (ov.set_overlay_alpha)(self.handle, a);
            },
            None => {
                unsafe {
                    let _ = (ov.hide_overlay)(self.handle);
                }
                self.shown_at = None;
            }
        }
    }
}

/// SteamVR Input(IVRInput)でボタン操作を受け取る。
///
/// このアプリは Overlay アプリなので、VRChat(シーンアプリ)が動いている間もアクションが届く。
/// 優先度 0 で使うので VRChat 側の入力は奪わない(同じ押下が両方に届く)。
/// SteamVR のダッシュボードを開いている間は届かない(SteamVR の仕様)。
struct SteamVrInput {
    action_set: ffi::VRActionSetHandle_t,
    actions: Vec<(InputAction, ffi::VRActionHandle_t)>,
    warned: bool,
}

const SET_MANIFEST_ATTEMPTS: u32 = 5;

impl SteamVrInput {
    fn init(lib: &OpenVrLibrary, manifest_path: &std::path::Path) -> Option<Self> {
        let input = lib.input()?;
        let path_c = CString::new(manifest_path.to_string_lossy().into_owned()).ok()?;
        // SteamVR がバインドの保存中などで忙しいと IPC エラー(7)で返ることがある(実機で確認)。
        // 少し待って何度かやり直す
        let mut err = 0;
        for attempt in 1..=SET_MANIFEST_ATTEMPTS {
            err = unsafe { (input.set_action_manifest_path)(path_c.as_ptr() as *mut c_char) };
            if err == 0 {
                break;
            }
            crate::log!(
                "[SteamVR] SetActionManifestPath 失敗 ({}/{}): EVRInputError={}",
                attempt,
                SET_MANIFEST_ATTEMPTS,
                err
            );
            if err != ffi::INPUT_ERROR_IPC || attempt == SET_MANIFEST_ATTEMPTS {
                return None;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        if err != 0 {
            return None;
        }

        let set_c = CString::new(input_manifest::ACTION_SET).unwrap();
        let mut action_set: ffi::VRActionSetHandle_t = 0;
        let err = unsafe {
            (input.get_action_set_handle)(set_c.as_ptr() as *mut c_char, &mut action_set)
        };
        if err != 0 {
            crate::log!("[SteamVR] GetActionSetHandle 失敗: EVRInputError={}", err);
            return None;
        }

        let mut actions = Vec::new();
        for action in InputAction::ALL {
            let name = CString::new(action.path()).unwrap();
            let mut handle: ffi::VRActionHandle_t = 0;
            let err =
                unsafe { (input.get_action_handle)(name.as_ptr() as *mut c_char, &mut handle) };
            if err != 0 {
                crate::log!(
                    "[SteamVR] GetActionHandle({}) 失敗: EVRInputError={}",
                    action.path(),
                    err
                );
                continue;
            }
            actions.push((action, handle));
        }
        crate::log!(
            "[SteamVR] ボタン割り当て(SteamVR Input)を有効化: {}",
            manifest_path.display()
        );
        Some(Self {
            action_set,
            actions,
            warned: false,
        })
    }

    /// SteamVR のボタン割り当て画面をこのアプリのページで開く。
    /// on_desktop = true なら PC の画面、false なら VR の中(ダッシュボード)
    fn open_binding_ui(&self, lib: &OpenVrLibrary, on_desktop: bool) {
        let Some(input) = lib.input() else {
            return;
        };
        let key = CString::new(manifest::APP_KEY).unwrap();
        let err = unsafe {
            (input.open_binding_ui)(
                key.as_ptr() as *mut c_char,
                self.action_set,
                ffi::INVALID_INPUT_VALUE_HANDLE,
                on_desktop,
            )
        };
        if err != 0 {
            crate::log!(
                "[SteamVR] OpenBindingUI(on_desktop={}) 失敗: EVRInputError={}",
                on_desktop,
                err
            );
        } else {
            crate::log!(
                "[SteamVR] OpenBindingUI(on_desktop={}) を呼んだ",
                on_desktop
            );
        }
    }

    /// 前回から「押された」(離された→押された)アクションを返す。毎フレーム呼ぶ
    fn poll(&mut self, lib: &OpenVrLibrary) -> Vec<InputAction> {
        let Some(input) = lib.input() else {
            return Vec::new();
        };
        let mut active = ffi::VRActiveActionSet_t {
            ul_action_set: self.action_set,
            ul_restricted_to_device: ffi::INVALID_INPUT_VALUE_HANDLE,
            ul_secondary_action_set: 0,
            un_padding: 0,
            n_priority: 0,
        };
        let err = unsafe {
            (input.update_action_state)(
                &mut active,
                std::mem::size_of::<ffi::VRActiveActionSet_t>() as u32,
                1,
            )
        };
        if err != 0 {
            if !self.warned {
                crate::log!("[SteamVR] UpdateActionState 失敗: EVRInputError={}", err);
                self.warned = true;
            }
            return Vec::new();
        }

        let mut pressed = Vec::new();
        for &(action, handle) in &self.actions {
            let mut data = ffi::InputDigitalActionData_t::default();
            let err = unsafe {
                (input.get_digital_action_data)(
                    handle,
                    &mut data,
                    std::mem::size_of::<ffi::InputDigitalActionData_t>() as u32,
                    ffi::INVALID_INPUT_VALUE_HANDLE,
                )
            };
            if err == 0 && data.b_active && data.b_changed && data.b_state {
                pressed.push(action);
            }
        }
        for a in &pressed {
            crate::log!("[SteamVR] ボタン操作: {}", a.label_ja());
        }
        pressed
    }
}

enum DashboardError {
    /// 同じキーのタブがもうある(このアプリの別のコピーが動いている)
    KeyInUse,
    Other(String),
}

fn create_dashboard_overlay(lib: &OpenVrLibrary) -> Result<VrOverlayHandle, DashboardError> {
    let key = CString::new(manifest::APP_KEY).unwrap();
    let name = CString::new(OVERLAY_NAME).unwrap();
    let mut main_handle: VrOverlayHandle = 0;
    let mut thumbnail_handle: VrOverlayHandle = 0;

    let err = unsafe {
        (lib.overlay().create_dashboard_overlay)(
            key.as_ptr() as *mut c_char,
            name.as_ptr() as *mut c_char,
            &mut main_handle,
            &mut thumbnail_handle,
        )
    };
    if err == ffi::OVERLAY_ERROR_KEY_IN_USE {
        return Err(DashboardError::KeyInUse);
    }
    if err != 0 {
        return Err(DashboardError::Other(format!("EVROverlayError={}", err)));
    }
    set_thumbnail_icon(lib, thumbnail_handle);
    Ok(main_handle)
}

/// ダッシュボード下段のタブのアイコン(サムネイル)にロゴを出す。失敗してもタブは使えるので報告だけ
fn set_thumbnail_icon(lib: &OpenVrLibrary, thumbnail_handle: VrOverlayHandle) {
    let Some(dir) = dirs::config_dir().map(|d| d.join("vrc-companion")) else {
        return;
    };
    let path = match manifest::write_icon_in(&dir) {
        Ok(p) => p,
        Err(e) => {
            crate::log!("[SteamVR] {}", e);
            return;
        }
    };
    let Ok(cpath) = CString::new(path.to_string_lossy().into_owned()) else {
        return;
    };
    let err = unsafe { (lib.overlay().set_overlay_from_file)(thumbnail_handle, cpath.as_ptr()) };
    if err != 0 {
        crate::log!("[SteamVR] タブのアイコン設定失敗: EVROverlayError={}", err);
    }
}

fn show_overlay(lib: &OpenVrLibrary, handle: VrOverlayHandle) -> Result<(), String> {
    let err = unsafe { (lib.overlay().show_overlay)(handle) };
    if err != 0 {
        return Err(format!("EVROverlayError={}", err));
    }
    Ok(())
}

fn set_overlay_input_method(lib: &OpenVrLibrary, handle: VrOverlayHandle) -> Result<(), String> {
    let err = unsafe { (lib.overlay().set_overlay_input_method)(handle, ffi::INPUT_METHOD_MOUSE) };
    if err != 0 {
        return Err(format!("EVROverlayError={}", err));
    }
    Ok(())
}

fn set_overlay_mouse_scale(
    lib: &OpenVrLibrary,
    handle: VrOverlayHandle,
    width: f32,
    height: f32,
) -> Result<(), String> {
    let mut scale = ffi::HmdVector2_t { v: [width, height] };
    let err = unsafe { (lib.overlay().set_overlay_mouse_scale)(handle, &mut scale) };
    if err != 0 {
        return Err(format!("EVROverlayError={}", err));
    }
    Ok(())
}

fn set_overlay_width_in_meters(
    lib: &OpenVrLibrary,
    handle: VrOverlayHandle,
    width_m: f32,
) -> Result<(), String> {
    let err = unsafe { (lib.overlay().set_overlay_width_in_meters)(handle, width_m) };
    if err != 0 {
        return Err(format!("EVROverlayError={}", err));
    }
    Ok(())
}

fn wait_frame_sync(lib: &OpenVrLibrary, timeout_ms: u32) -> Result<(), String> {
    let err = unsafe { (lib.overlay().wait_frame_sync)(timeout_ms) };
    if err != 0 {
        return Err(format!("EVROverlayError={}", err));
    }
    Ok(())
}

/// クリック成立時にレーザーポインタへ短い振動フィードバックを返す。
/// 継続時間・周波数・振幅は「トリガー押下がUIに反映された」ことが分かる程度の
/// 短いパルスとして適当に選んだ値(実機での感触調整が必要な可能性あり)。
fn trigger_laser_mouse_haptic_vibration(
    lib: &OpenVrLibrary,
    handle: VrOverlayHandle,
) -> Result<(), String> {
    let err =
        unsafe { (lib.overlay().trigger_laser_mouse_haptic_vibration)(handle, 0.05, 40.0, 1.0) };
    if err != 0 {
        return Err(format!("EVROverlayError={}", err));
    }
    Ok(())
}

fn set_overlay_texture(
    lib: &OpenVrLibrary,
    handle: VrOverlayHandle,
    texture: &mut Texture_t,
) -> Result<(), ffi::EVROverlayError> {
    let err = unsafe { (lib.overlay().set_overlay_texture)(handle, texture) };
    if err != 0 {
        return Err(err);
    }
    Ok(())
}
