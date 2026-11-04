<p align="center">
  <img src="assets/voxtype.svg" width="128" alt="voxtype">
  &nbsp;&nbsp;&nbsp;
  <img src="assets/logo.svg" width="128" alt="VRC Companion">
</p>

# voxtype (PC) + VRC Companion (SteamVR)

VRChat で遊んでるときにあると便利なあれこれ

- **voxtype**: PC 側の常駐ツール。音声をタイピング無しで入力・翻訳・校正できる
- **VRC Companion**: SteamVR 側のオーバーレイ。voxtype を VR の中から操作する

## 概要

音声入力(音声認識)したテキストを、Grok による校正・翻訳・質問応答を挟んで、クリップボードコピー・アクティブウィンドウへの自動入力・VRChat チャットボックスへの送信、から好きな組み合わせで出力できる常駐ツールです。

主な機能:

- 🎙 音声認識 (xAI Speech-to-Text による日本語ストリーミング認識)
- 🧹 校正 (Grok): フィラー除去・言い間違い修正。オン/オフ切り替え可
- 🌐 翻訳 (Grok): 認識結果を翻訳して `{source}/{translated}` 形式で出力
- 🤖 質問 (Grok): 認識結果をそのまま質問として AI に投げ、回答を `> {source}\nAI> {answer}` 形式で出力
- 📋 出力をクリップボードへ自動コピー
- ⌨️ 出力をアクティブウィンドウへ自動入力(貼り付け or 直接入力、Enter 送信の有無も選択可)
- 💬 出力を VRChat のチャットボックスへ送信 (OSC)
- 📝 QvPen 呼び出しボタン

翻訳・質問・そのまま出力(エコー)は排他のモードとして切り替えます。出力フォーマットは Settings から自由に変更できます(プレースホルダは `{source}` `{translated}` `{answer}`)。

## 必要なもの

- [xAI API Key](https://x.ai/) — 音声認識と Grok (校正・翻訳・質問) に使用
- VRChat (OSC を有効にしておくこと。ポート `9000` へ送信します)

## セットアップ

1. アプリを起動する
2. 右上の「⚙ Settings」を開く
3. xAI API Key を入力して保存する

設定は以下のファイルに保存されます。

- Linux: `~/.config/vrc-companion/config.json`
- Windows: `%APPDATA%\vrc-companion\config.json`
- macOS: `~/Library/Application Support/vrc-companion/config.json`

## 使い方

1. 「⏺ Start」ボタンで録音開始。無音が続くと自動で録音停止して認識結果が確定します
2. 各チェックボックスで結果の送り先を選択
   - 「Auto-input to active window」と「Send to VRChat」は排他(同時には片方だけ有効)
3. 「校正 (Grok)」のオン/オフと、モード(エコー/翻訳/質問)を選択
   - 翻訳モードでは翻訳先の言語も選べます
4. 「📝 call QvPen」ボタンで QvPen を呼び出せます

## VR の中の表示（SteamVR オーバーレイ）

SteamVR のダッシュボードに「VRC Companion」のタブが出て、録音の開始・停止、送り先の切り替え、QvPen の呼び出しができます。

SteamVR はあとから起動しても大丈夫です（起動したら数秒で自動でつながります）。SteamVR を終了すると、このアプリも一緒に終了します。このアプリが SteamVR を起動することはありません。

VR のパネル右端の丸いボタン A/B/X/Y は、アバターのパラメーターをオン/オフするボタンです。Settings の「VR のボタン A/B/X/Y の設定…」で、パラメーター名・型・オンとオフの値を決めてください。名前は `Hat` でも `/Hat` でも `/avatar/parameters/Hat` でもよく、どれも `/avatar/parameters/Hat` に送ります（末尾の / と // は不可）。VRChat の OSC を有効にしておく必要があります。

表示の大きさは設定ファイル（上の `config.json`）の `steamvr_overlay_width_m` で変えられます（幅をメートルで。既定は `1.92`）。変えたらアプリを起動しなおしてください。

## コントローラーのボタンで操作する

VRChat を遊んでいる最中でも、コントローラーのボタンで録音を始めたりできます（SteamVR のボタン割り当て機能を使います）。

はじめは、左の十字キー下（Steam Frame）、左 A（Index）、左 X（Quest）をすばやく2回押すと録音が始まります。「録音の開始/停止」「QvPen を呼ぶ」は、はじめは割り当てられていません。

割り当てを見たり、好きなボタンに変えたりするとき:

1. VR の中の「VRC Companion」タブ、またはデスクトップの Settings の「キー割り当て変更」を押す
2. SteamVR の割り当て画面で「録音を開始」「録音の開始/停止」「QvPen を呼ぶ」を好きなボタンに割り当てる（1回押し・2回押し・長押しなども選べます）

SteamVR のダッシュボードを開いている間はボタンが効きません。

録音が始まったとき・止まったときは、目の前の少し下に「録音開始」「録音終了」が約2秒出ます（ダッシュボードを閉じていても見えます）。

## ビルド・実行

VRChat は Windows で動くため、本ツールも Windows ネイティブ実行ファイルとしてビルドします。

```sh
make build   # Windows 用リリースビルド (x86_64-pc-windows-gnu)
make test    # テスト実行
make clean   # ビルド成果物を削除
```

生成物: `target/x86_64-pc-windows-gnu/release/vrc-companion.exe`

WSL からは `\\wsl.localhost\<ディストロ名>\...\vrc-companion.exe` として Windows 側からアクセスできます。

詳細は開発者向けに `Makefile` を参照してください。
