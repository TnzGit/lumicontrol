# LumiControl

<p align="center"><strong>周囲の明るさに合わせて Windows モニターの輝度を自動調整。</strong></p>

<p align="center">
  <a href="../../README.md">English</a> |
  <a href="README.zh-CN.md">简体中文</a> |
  <a href="README.zh-TW.md">繁體中文</a> |
  日本語 |
  <a href="README.ko.md">한국어</a> |
  <a href="README.es.md">Español</a> |
  <a href="README.pt-BR.md">Português</a> |
  <a href="README.tr.md">Türkçe</a> |
  <a href="README.ru.md">Русский</a> |
  <a href="README.uk.md">Українська</a>
</p>

LumiControl は Windows 向けの環境光連動型モニター輝度コントローラーです。
ESP32-C3 と GY-302/BH1750 センサーから照度を取得し、軽量なバックグラウンド
Agent が設定したキャリブレーションカーブに従って DDC/CI 対応モニターを滑らかに
調整します。オプションのリレーファームウェアでは、デスクやモニター背面の低電圧
ライトストリップも制御できます。

## ダウンロード

最新の Windows x64 インストーラーは
**[GitHub Releases](https://github.com/TnzGit/lumicontrol/releases)** から
ダウンロードできます。

> [!WARNING]
> 現在の Preview インストーラーにはコード署名がありません。Windows SmartScreen
> に「不明な発行元」と表示される場合があります。実行前に Release に掲載された
> SHA-256 チェックサムを確認してください。

### 必要な環境

- Windows 10 または Windows 11（x64）
- OSD メニューで DDC/CI を有効にしたモニター
- ESP32-C3 SuperMini と GY-302/BH1750 環境光センサー
- ライト制御を行う場合は、対応する 5 V リレーモジュール

インストール後に USB センサーを接続して LumiControl を開きます。対応ハードウェアは
自動検出されます。「Calibration」で部屋とモニターに合う照度・輝度カーブを設定します。

## 主な機能

- 常駐する軽量 Windows Agent と、必要なときだけ開く Tauri UI
- センサーのみ／センサー＋リレー構成の USB 自動検出
- 動作中でも目標を変更できる滑らかな DDC/CI 輝度遷移
- ドラッグ操作対応の照度カーブと 3 段階の元に戻す履歴
- 手動輝度変更の検出とモニターごとのキャリブレーション
- 時刻、日の出、日没、天気、照度、モニター輝度を使う優先順位付きライト規則
- NO/NC 接点の反転、手動ライト操作、フォールバック動作
- ライト、ダーク、システム連動テーマ
- 機密性のあるハードウェア識別子を伏せたローカル診断

## ハードウェア配線

検証済みの ESP32-C3 SuperMini 配線：

| 機能 | ESP32-C3 ピン |
| --- | --- |
| GY-302/BH1750 SDA | GPIO4 |
| GY-302/BH1750 SCL | GPIO5 |
| リレーモジュール入力 | GPIO10 |

`sensor` と `sensor-relay` の 2 種類のファームウェアプロファイルがあります。
リレープロファイルは一般的なアクティブ Low の 5 V モジュールを想定しています。
ドライバーとフライバック保護を備えたモジュールを使用し、裸のリレーコイルを ESP32
GPIO から直接駆動しないでください。商用電源配線は低電圧回路から隔離してください。

## プライバシーとライセンス

Agent と UI は、現在の Windows ユーザーだけが利用できる名前付きパイプで通信します。
設定、バックアップ、ログ、診断は `%LOCALAPPDATA%\LumiControl` に保存されます。
有効なルールで天気情報が必要な場合だけ天気リクエストを行い、アカウントは不要です。

LumiControl は [PolyForm Noncommercial License 1.0.0](../../LICENSE) の下で
非商用利用向けにソースを公開しています。**商用利用は許可されておらず**、OSI が定義する
オープンソースライセンスではありません。第三者コンポーネントには各ライセンスが適用されます。

ビルド手順、構成、ファームウェアの詳細は[英語版 README](../../README.md)と
[`docs/v2`](../v2/)を参照してください。
