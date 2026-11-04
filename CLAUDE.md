# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project origin

This project is derived from [winh](~/git/winh/) (a sibling Rust/egui voice-transcription app for the same author). vrc-companion re-extracted a minimal subset of winh's features and rebuilt them with TDD. When in doubt about *why* something is structured a certain way, winh is the reference implementation to diff against — but note the STT client here is a **streaming websocket** client, whereas winh's records a full clip and uploads a WAV; do not assume behavior transfers 1:1.

## Commands

```sh
make build   # release build for Windows (x86_64-pc-windows-gnu) — the actual target platform
make install # build, then copy the .exe to ~/apps/vrc-companion
make run     # install, then run the copied .exe (only works if the exe can execute, i.e. on/via Windows)
make test    # cargo test
make fmt     # cargo fmt
make lint    # cargo clippy --all-targets -- -D warnings
make clean   # cargo clean
```

Run a single test: `cargo test <test_name>` (e.g. `cargo test test_auto_input_and_vrchat_are_mutually_exclusive`). Tests are plain `cargo test` — no target flag needed, they run on the host (Linux/WSL), not the Windows cross-target.

VRChat, SteamVR overlay, and the GUI only run meaningfully on Windows. This repo is developed from WSL and cross-compiled; there is no GPU passthrough in this WSL environment, so the GUI cannot be visually verified from `cargo run` directly here — `make build` (or `make install`) and run the `.exe` on Windows (accessible via `\\wsl.localhost\<distro>\...\vrc-companion.exe`, or `~/apps/vrc-companion/vrc-companion.exe` after `make install`) to see the actual UI.

SteamVR overlay / OpenVR FFI / WGL code is `#[cfg(windows)]`. Host `cargo test` / `cargo build` never compiles those files; always also run `cargo build --target x86_64-pc-windows-gnu` when touching `src/steamvr/`.

`make lint` (`clippy -- -D warnings`) currently fails on a pre-existing warning set (dead_code on Windows-gated types in host builds, `too_many_arguments`, `field_reassign_with_default` in tests, etc.). Do not treat that as a regression unless the set grows. New clippy errors in code you touch should still be fixed.

## Architecture

Single-window `eframe`/`egui` immediate-mode app. All mutable state lives in the `App` struct in `src/main.rs`; `App::update()` runs every frame and does two things before drawing: drains any pending channel messages, then renders. There is no separate event loop or message dispatcher — cross-thread communication is entirely channel-based:

- `transcription_receiver` (`tokio::mpsc`): partial/success/error events streamed from the STT websocket task, running on its own `tokio::runtime::Runtime` spawned per recording session (`App::start_streaming_transcription`) and shut down (`rt.shutdown_background()`) once transcription completes or errors.
- `process_receiver` (`std::sync::mpsc`): Grok HTTP call (correct/translate/question, `integrations::grok::GrokClient`, blocking `reqwest`) runs on a plain spawned thread, result polled once per frame.
- SteamVR overlay (`OverlayHandle`): `snapshot_tx` (App → overlay thread) and `action_rx` (overlay thread → App). See below.

### Data flow per utterance

1. `AudioRecorder` (`src/audio/mod.rs`) captures mic input via `cpal`, downmixes to mono, chunks into ~100ms `Vec<f32>` buffers pushed through an unbounded channel, and independently tracks silence duration + peak amplitude for the UI meter. Recording auto-stops when `is_silent(silence_duration_secs)` goes true (checked every frame while recording), with a 3s grace period after start.
2. `SpeechToTextClient::stream_transcribe` (`src/audio/speech_to_text.rs`) owns the xAI STT websocket (`wss://api.x.ai/v1/stt`) for the session: waits for `transcript.created`, streams PCM16 audio frames as they arrive, sends `audio.done` once the audio channel closes (recording stopped), and forwards `transcript.partial`/`transcript.done` events back as `TranscriptionMessage`.
3. On `TranscriptionMessage::Success`, `App::on_transcription_success` runs the processing pipeline: optional proofread (Grok correct) → mode (`Echo` as-is / `Translate` via Grok / `Question` via Grok with Live Search) → output format template (`{source}` `{translated}` `{answer}` placeholders). Echo without proofread dispatches synchronously; anything needing Grok runs on a spawned thread and the formatted result lands via `process_receiver`. On Grok failure the raw text is dispatched as-is.
4. `App::dispatch_output` sends the final text to whichever sinks are enabled in `Config`: clipboard (`arboard`), VRChat OSC (`integrations::vrchat`), and/or active-window auto-input (`integrations::auto_input`, via `enigo`).

### Config invariants (`src/config.rs`)

One exclusive pair — only ever flip them via the exclusive helpers, which force the other off:

- `auto_input_enabled` ↔ `vrchat_enabled` via `enable_auto_input_exclusive()` / `enable_vrchat_exclusive()`

`proofread_enabled` and `mode` (`Echo`/`Translate`/`Question`) are independent of the sink toggles. Config is a flat JSON struct persisted at the OS config dir (`~/.config/vrc-companion/config.json` on Linux) via `Config::load`/`Config::save`; every field has a `#[serde(default = ...)]` so old config files never fail to deserialize when new fields are added — keep that pattern when adding settings. `migrate_and_parse` drops removed keys (`eliza_*`) and maps the old `auto_translate_enabled: true` to `mode = "translate"`.

### SteamVR overlay (`src/steamvr/`)

Desktop GUI stays; a SteamVR **dashboard overlay** mirrors the operational controls (checkboxes, Start/Stop, QvPen, proofread toggle, mode selection, last transcription / output text). Settings stay on the desktop window only.

**Why custom FFI, not the `openvr` / `ovr_overlay` crates:** the official C++ SDK (and crates wrapping it) uses the MSVC vtable ABI, which is incompatible with this project's cross-compile target `x86_64-pc-windows-gnu`. Instead we `libloading`-load `openvr_api.dll` and call `VR_GetGenericInterface("FnTable:IVROverlay_028", ...)` / `"FnTable:IVRApplications_008"` / `"FnTable:IVRInput_011"` for ABI-safe function-pointer tables. Field order/count is ABI-critical and transcribed verbatim from [openvr_capi.h](https://github.com/ValveSoftware/openvr/blob/master/headers/openvr_capi.h); unused slots are `PlaceholderFn`. Tripwire tests lock table field counts and struct sizes.

**Why custom WGL, not glutin/winit:** `eframe` already owns the process's only `winit::EventLoop`; creating a second one fails on every platform. `GlOverlayRenderer` (`render.rs`) builds a hidden window + WGL context via `windows-sys`, loads GL through `wglGetProcAddress` (fallback: `opengl32.dll` `GetProcAddress`), and paints egui into an FBO via `egui_glow`. WGL contexts have thread affinity — create/render/destroy stay on the overlay thread spawned by `session::start`.

**Do not couple overlay UI state to Config directly.** Overlay thread is given an `OverlaySnapshot` (copy of the flags + last texts + `is_recording`). Clicks become `OverlayAction`s sent back to `App`, which applies them through the same exclusive helpers, `config.save()`s, and `push_steamvr_snapshot()`s. Config remains the single source of truth.

**Input will not work unless `SetOverlayInputMethod(Mouse)` is called** after `ShowOverlay`. Default is `InputMethod_None`; the compositor then never generates `MouseMove` / `MouseButtonDown` / `Up` even if `PollNextOverlayEvent` is wired correctly. Also call `SetOverlayMouseScale` so `VREvent_Mouse_t.x/y` match the render size. Laser-pointer clicks are always mapped to `egui::PointerButton::Primary` (dashboard trigger = click). Overlay mouse Y is **not** inverted (an early invert was wrong on hardware).

**`App::update` must keep ticking while the overlay is alive.** eframe is reactive and otherwise skips `update()` when the desktop window is unfocused (the usual case while the user is in VR). Overlay clicks then appear to "flash and revert" because the overlay thread keeps redrawing the stale snapshot. `if self.steamvr_overlay.is_some() { ctx.request_repaint_after(100ms); }` exists specifically for this — do not remove it.

Pacing is `WaitFrameSync` (compositor frame sync, not a fixed 33ms timer). Successful clicks trigger a short laser-mouse haptic. Overlay init failure (SteamVR not running, DLL missing) is non-fatal — `steamvr::start()` returns `None`.

SteamVR auto-launch is registered every successful overlay init via `IVRApplications` (`AddApplicationManifest` + `SetApplicationAutoLaunch`). Manifest JSON is built in `manifest.rs` (host-testable; not Windows-gated). Overlay key and app key share `manifest::APP_KEY` (`"cympfh.vrc_companion"`) — OpenVR requires they match. This is idempotent side-effect, not a user-facing toggle.

**SteamVR Input (button bindings) is the only recording trigger from the controller** (the old OSC MuteSelf listener on port 9001 was removed in 0.5.0; OSC is now send-only, chatbox on 9000). `input_manifest.rs` (host-testable) builds an action manifest (`/actions/main/in/start_recording`, `toggle_recording`, `call_qvpen`, all boolean) plus default bindings for `frame_controller` / `knuckles` / `oculus_touch` (double-click on VRChat's default mic button: left dpad_down / left A / left X; VRChat has no Frame binding, SteamVR remaps its touch/knuckles ones to dpad_down via `frame_controller_remapping.json`). `session::run` writes them to `%APPDATA%\vrc-companion\steamvr_input\` every start, puts `action_manifest_path` in the vrmanifest, calls `IdentifyApplication(std::process::id(), APP_KEY)` (so a manually launched exe still gets our app key / bindings; passing 0 as the header suggests made vrserver set the app PID to 0 on hardware), then `SetActionManifestPath` before the first event poll. Each overlay tick: `UpdateActionState` with priority **0** + `GetDigitalActionData`, rising edge (`bActive && bChanged && bState`) → `OverlayAction::from_input` → same `action_tx`. Presses are logged (`[SteamVR] ボタン操作: …`). The app does not read back or display the current bindings (removed on purpose: the overlay should stay small; SteamVR's own binding UI shows them). `OpenBindingUI(APP_KEY, set, 0, on_desktop)` runs on the overlay thread: the overlay's small header button 「キー割り当て」 (left of the status pill) (`OverlayAction::OpenBindingUi`, handled locally, never forwarded) uses `false` (in VR); the desktop Settings button of the same name sends `OverlayCommand::OpenBindingUi` through `OverlayHandle::command_tx` and uses `true` (desktop). An empty `frame_hmd` binding exists only to silence SteamVR's "has no configured binding" log spam. Overlay apps receive actions while the scene app (VRChat) has focus; priority 0 does not steal input from the game (the higher `k_nActionSetOverlayGlobalPriority*` range would, and needs a SteamVR developer setting — deliberately not used). Actions do not fire while the SteamVR dashboard is open. `IVRInput` is fetched as `FnTable:IVRInput_011`, falling back to `_010`; the entries called (SetActionManifestPath … OpenBindingUI) sit at the same positions in both. All of it is non-fatal.

**Connection lifecycle (never starts SteamVR).** `session::start` always spawns the thread; `supervise` loops: if `runtime_watch::steamvr_running()` (Toolhelp snapshot, looks for `vrserver.exe` only) is false it waits `RETRY_EVERY` (5 s) — calling `VR_InitInternal` as an Overlay app while SteamVR is down would launch SteamVR, so it is never called then. Otherwise `OpenVrLibrary::load()` (VR_Init; on any later setup failure it calls `VR_ShutdownInternal` before returning) → `run_session` (writes input files, registers auto-launch, SteamVR Input, dashboard overlay, renderer, toast) → on exit everything is dropped and `drop(lib)` does `VR_ShutdownInternal` + unloads the DLL, then `SessionEnd::after()` decides: SteamVR quit / vrserver gone / another instance → `ExitApp` (sets `OverlayHandle::exit_requested` + `request_repaint`; `App::update` stops recording without waiting for transcription, `shutdown_background`s the tokio runtime, flushes stdout/stderr and sends `ViewportCommand::Close`, so the whole process exits), setup failure → `Retry` (wait 5 s, try again), app closed → thread just ends. Waiting for SteamVR at startup stays (5 s polling); only "stay alive after SteamVR quits" was removed in 0.5.1. `run_session` ends on: `VREvent_Quit` from `IVRSystem_026::PollNextEvent` (drained every tick, before other OpenVR calls) → `AcknowledgeQuit_Exiting` then return immediately so `VR_Shutdown` follows (SteamVR kills processes that keep vrclient connected after Quit — that was the 0xc0000409 crash ~10 s after SteamVR closed; the process itself may keep running, per Joe Ludwig in openvr#1061); `vrserver.exe` gone (checked every 1 s, no ack); snapshot channel closed (app exit); or setup failure. Status lines are logged only on change (`StatusLog`). `OverlayHandle::connected` (AtomicBool) gates the desktop 「キー割り当て変更」 button and the 100 ms repaint (none while disconnected: the thread calls `request_repaint` when connected/exit flags change, via the `egui::Context` passed to `steamvr::start`). Only `PollNextEvent` and `AcknowledgeQuit_Exiting` of the IVRSystem table are real; if the table can't be fetched, the vrserver check still catches the shutdown.

Every successful connect re-writes `%APPDATA%\vrc-companion\vrc-companion.vrmanifest`, calls `AddApplicationManifest` + `SetApplicationAutoLaunch(true)` (logs 「SteamVR の自動起動に登録した」), so SteamVR's startup list heals itself after one manual launch with SteamVR running. If `CreateDashboardOverlay` returns `KeyInUse` (17) another copy already owns the tab (e.g. manual launch + SteamVR auto-launch) → `SessionEnd::AnotherInstance`, this copy exits (0.5.1; before that its desktop GUI kept running). **Tests must never write or delete real files under `%APPDATA%`**: an old manifest test wrote and then deleted the real `.vrmanifest`; running the Windows test exe on the user's PC removed it, SteamVR dropped the app from `appconfig.json` ("Unable to read manifest. Removing that file") and auto-launch stopped. `write_manifest_in(dir, …)` exists so the test uses a temp dir.

**Recording toast.** `session::run` also owns a second, ordinary (non-dashboard) overlay `<APP_KEY>.toast`, placed with `SetOverlayTransformTrackedDeviceRelative` on the HMD (`toast_ui::TOAST_OFFSET_M`, 0.42 m wide, sort order 100), so it is visible while the dashboard is closed. It is shown when `OverlaySnapshot::is_recording` flips (`ToastKind::from_transition`), drawn once by its own `GlOverlayRenderer` via `render_ui` (texture set right after drawing, while that renderer's GL context is current), and faded each tick with `SetOverlayAlpha` (`toast_ui::alpha_at`: 0.15 s in, hold, gone at 2.2 s → `HideOverlay`). Failure to create it is logged and ignored. `--dump-overlay` also writes `-toast-start` and `-toast-stop`.

The overlay renderer has its **own** `egui::Context` and must load `fonts/NotoSansJP-Regular.ttf` itself (`setup_fonts` in `render.rs`). The desktop window's font setup in `main.rs` does not apply to it; skipping this produces tofu for Japanese.

### Module layout

- `src/audio/` — mic capture + STT client (things that touch raw audio samples)
- `src/integrations/` — everything that talks to an external system by sending already-transcribed text (VRChat OSC, Grok HTTP via `grok.rs`, OS-level auto-input/QvPen via `enigo`/`windows-sys`)
- `src/steamvr/` — SteamVR dashboard overlay
  - `bridge.rs` — all OS: `OverlaySnapshot` / `OverlayAction` / `OverlayHandle` / `start()` (non-Windows always `None`) / `overlay_fields()`
  - `ffi.rs` — `#[cfg(windows)]`: raw OpenVR C API tables
  - `session.rs` — `#[cfg(windows)]`: overlay lifecycle, event poll, texture submit, auto-launch register
  - `render.rs` — `#[cfg(windows)]`: WGL + `egui_glow` offscreen painter (texture = logical size × `PIXELS_PER_POINT`)
  - `overlay_ui.rs` — all OS: the overlay's egui UI (cards, switches, buttons). Host-tested by clicking every hit target. Mouse scale = logical size; width in meters comes from `Config::steamvr_overlay_width_m` (default 2.4)
  - `dump.rs` — `#[cfg(windows)]`: `vrc-companion.exe --dump-overlay <prefix>` renders sample snapshots to BMP through the real WGL path (no SteamVR needed)
  - `manifest.rs` — all OS: `.vrmanifest` JSON (host-testable)
  - `input_manifest.rs` — all OS: SteamVR Input action manifest + default bindings (host-testable)
  - `runtime_watch.rs` — all OS: is SteamVR (vrserver.exe) running, session-end reasons, log de-dup (host-testable)
  - `toast_ui.rs` — all OS: recording start/stop toast drawing + fade curve (host-testable)
- `src/config.rs`, `src/main.rs` — stay at the root

`auto_input::call_qvpen` and the Windows-`FindWindowW`/`SetForegroundWindow` calls in `integrations/auto_input.rs` are `#[cfg(windows)]`-gated; the non-Windows path just returns an error, since this only matters on the deployed target.

Windows-gated types/fns that are only constructed from `#[cfg(windows)]` code (`OverlayAction` variants, `write_manifest`, …) will trip `dead_code` on host builds. That is expected; `#[allow(dead_code)]` at those sites is the established pattern, not leftover junk. Do not "clean them up" by deleting the allow or the type.

## Process notes

- Development follows TDD — tests are colocated with the code they cover (`#[cfg(test)] mod tests` at the bottom of each file), not in a separate test tree.
- AFK toggle (`/input/AFKToggle`) was implemented, found unsupported on hardware, and **fully removed** on user instruction. Do not re-add it.
