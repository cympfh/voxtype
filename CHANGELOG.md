# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.7.0] - 2026-11-04

### Added

- 校正 (Grok): フィラー除去・言い間違い修正をしてから出力する。オン/オフを切り替えられる
- 質問モード (Grok): 認識結果をそのまま質問として AI に投げ、回答を出力する。Live Search (Web / X) つき
- 出力フォーマットを Settings から変更できる。既定は翻訳が `{source}/{translated}`、質問が `> {source}\nAI> {answer}`。プレースホルダは `{source}` `{translated}` `{answer}`
- VR のオーバーレイに「加工」カードを追加し、校正のオン/オフとモード(エコー/翻訳/質問)を VR からも切り替えられる

### Changed

- 翻訳は Eliza サーバーではなく xAI の Grok (grok-4.7) で行う。翻訳と質問は排他のモード選択になり、常時出力されるのは1系統だけ
- 認識結果の処理は「校正(任意) → モード(エコー/翻訳/質問) → フォーマット → 出力先」の1本のパイプラインになった
- VRChat に送るテキストの先頭に付けていた「> 」をやめた。見せ方はフォーマットで自由に変えられる

### Removed

- Eliza サーバーへの依存を削除。Eliza Agent URL の設定、「Send to Eliza」、Eliza 応答の VRChat 送信を廃止
- 設定ファイルの `eliza_*` キーは読み込み時に破棄される。旧 `auto_translate_enabled: true` はモード「翻訳」として引き継ぐ

## [0.6.4] - 2026-10-20

### Fixed

- WSL などから起動すると、数分後に VR のオーバーレイが消えて戻らなかった。標準エラー出力が使えなくなったときにログの 1 行がもとで VR の処理が落ちていた。書けなくても無視するようにした
- VR の処理が落ちると、PC の画面は動いたままオーバーレイだけが黙って消え、戻らなかった。落ちたら 3 秒後につなぎ直し、PC の画面に「⚠ VR の処理が落ちたので、つなぎ直しています」と出す

### Added

- 落ちたときの理由（メッセージ・場所・バックトレース）をログに残す
- WaitFrameSync の失敗は 1 分に 1 回だけ、その間の回数を添えてログに出す

## [0.6.3] - 2026-10-20

### Added

- ログを `%APPDATA%\vrc-companion\vrc-companion.log` に時刻付きで書く（どう起動しても残る）。前回の分は `vrc-companion.log.1` に残る。1MB を超えたらそこで回す

### Changed

- アイコン PNG は中身が同じなら書き直さない。書けなくても前のファイルがあればアプリ一覧のアイコンに使う

## [0.6.2] - 2026-10-20

### Added

- ロゴ。README の先頭、Windows のアプリのアイコン（exe・ウィンドウ・タスクバー）、SteamVR のダッシュボードのタブとアプリ一覧のアイコンに出る

## [0.6.1] - 2026-10-20

### Fixed

- VR のパネルで、トリガーを押している間にレーザーポインタが少しでも動くと、ボタンやスイッチが押せなかった。押した部品の上で離せば、手ぶれがあってもゆっくり押してもクリックになる

## [0.6.0] - 2026-10-20

### Added

- VR のパネル右端に丸いボタン A/B/X/Y。押すたびにオン/オフが切り替わり、VRChat のアバターパラメーター `/avatar/parameters/<名前>` に OSC（127.0.0.1:9000）で値を送る。起動時はすべてオフ
- PC の Settings →「VR のボタン A/B/X/Y の設定…」で、ボタンごとにパラメーター名・型（Bool / Int 0〜255 / Float -1.0〜1.0）・オンの値・オフの値を決める。config.json の `avatar_buttons` に保存。名前が空のボタンは VR で薄く表示され、押しても何もしない

## [0.5.2] - 2026-10-20

### Changed

- VR の中のパネルの既定の大きさを 80% に（`steamvr_overlay_width_m` の既定を 2.4 → 1.92）
- VR の中の「キー割り当て変更」ボタンを、1 行まるごと使う大きなボタンから、右上の「待機中」の左にある小さな「キー割り当て」ボタンに。パネルの高さもその分短く

## [0.5.1] - 2026-10-19

### Changed

- SteamVR を終了したら（または SteamVR が落ちたら）、このアプリも一緒に終了するように（デスクトップのウィンドウも閉じる）。録音中だった場合は録音を止めてから終了する
- SteamVR より先にアプリを起動したときは、これまでどおり SteamVR の起動を待ち、起動したら自動でつなぐ。このアプリが SteamVR を起動することはない
- 同じアプリがもう SteamVR につながっているときに2つ目を起動すると、2つ目は終了する

## [0.5.0] - 2026-10-18

### Added

- コントローラーのボタンで操作できるように（SteamVR のボタン割り当て機能）。VRChat を操作中でも効く。割り当てられる操作は「録音を開始」「録音の開始/停止」「QvPen を呼ぶ」。1回押し・2回押し・長押しなど押し方も選べる
- 既定の割り当て: 左の十字キー下（Steam Frame）、左 A（Index）、左 X（Quest）を2回押すと「録音を開始」。「録音の開始/停止」「QvPen を呼ぶ」は割り当てなし。SteamVR の割り当て画面で変えた場合はそちらが使われる
- 録音の開始/終了を VR の中にトーストで表示（目の前の少し下に約2秒。ダッシュボードを閉じていても見える）
- VR の中のオーバーレイと Settings に「キー割り当て変更」ボタン: SteamVR の割り当て画面を開く（オーバーレイからは VR の中、Settings からは PC の画面）

### Fixed

- SteamVR を終了すると、数秒後にこのアプリが落ちていた。SteamVR の終了の知らせを受けて切断し、アプリはそのまま動き続けるように
- SteamVR より先にアプリを起動すると、VR の表示がずっと出なかった（そのうえ SteamVR を起動してしまうことがあった）。SteamVR の起動を待ち、起動したら自動でつなぐように。このアプリが SteamVR を起動することはない

### Removed

- VRChat の OSC でミュート2回を受け取って録音を始める仕組み（ポート 9001 の受信）。上のボタン割り当てで置き換え。VRChat のチャットボックスへの送信（OSC、ポート 9000）はこれまでどおり

## [0.4.0] - 2026-10-18

### Changed

- SteamVR オーバーレイを約2倍の大きさに。表示幅を 2.4m に指定し（以前は指定なしで SteamVR 既定の 1m）、UI は 600×620 の論理サイズを 2 倍の解像度（1200×1240）で描くので文字がくっきり
- オーバーレイの見た目を一新。暗い背景にカード、Start/Stop と QvPen の大きなボタン、チェックボックスの代わりにスイッチ、録音中の表示、指したとき・押したときの色の変化。長い文章は数行で「…」
- 機能・ラベル・押したときの動作は今までどおり

### Added

- 設定 `steamvr_overlay_width_m`（オーバーレイの表示幅、既定 2.4、0.5〜6.0）
- `vrc-companion.exe --dump-overlay <接頭辞>`: SteamVR なしでオーバーレイの見た目を BMP に書き出す（確認用）

## [0.3.0] - 2026-09-03

### Added

- Eliza 経由の自動翻訳 (`auto_translate_enabled`)。VRChat 送信後に翻訳結果を `{原文} / {翻訳}` で再送信。通常会話 (`eliza_enabled`) とは排他
- SteamVR ダッシュボードオーバーレイ (`src/steamvr/`)。OpenVR C API を `libloading` で動的ロードし、隠し WGL コンテキスト + `egui_glow` で描画
- オーバーレイからチェックボックス操作・Start/Stop 録音・QvPen 呼び出しが可能。レーザーポインタ入力 (`SetOverlayInputMethod(Mouse)`) とクリック時ハプティクス
- オーバーレイにタイトル、最終文字起こし、Eliza 応答 / 翻訳結果を表示
- SteamVR 起動時の自動起動登録 (`IVRApplications` + `.vrmanifest`)。キーは `cympfh.vrc_companion`

### Changed

- 設定チェックボックスのラベルを統一
- `make run` は `make install` 経由で `~/apps/vrc-companion/vrc-companion.exe` を実行する

### Fixed

- オーバーレイの Y 座標反転 (実機で上下が逆だった)
- デスクトップウィンドウ非フォーカス時にオーバーレイのクリックが一瞬で戻る問題 (`request_repaint_after`)

(git commit: 3f82667)

## [0.2.0] - 2026-07-01

### Added

- MuteSelf の false→true 切り替え(1秒以内)を検知して録音を自動開始する機能(winh から移植、判定ロジックは `MuteToggleDetector` として切り出しテスト可能にした)

### Changed

- `src/` を `audio/`, `integrations/` サブディレクトリに再編
- CLAUDE.md を追加(アーキテクチャ・開発コマンドの説明)

(git commit: 6f9c34d)

## [0.1.0] - 2026-07-01

### Added

- xAI Speech-to-Text によるストリーミング音声認識(日本語)
- 認識結果のクリップボード自動コピー
- 認識結果のアクティブウィンドウへの自動入力(貼り付け/直接入力、Enter 送信オプション)
- 認識結果の VRChat チャットボックスへの OSC 送信
- 認識結果の Eliza(対話エージェント)への送信、応答の GUI 表示、応答の VRChat への送信
- QvPen 呼び出しボタン

(git commit: e461a3b)
