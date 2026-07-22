# LumiControl

<p align="center"><strong>依照環境光線自動調整 Windows 顯示器亮度。</strong></p>

<p align="center">
  <a href="../../README.md">English</a> |
  <a href="README.zh-CN.md">简体中文</a> |
  繁體中文 |
  <a href="README.ja.md">日本語</a> |
  <a href="README.ko.md">한국어</a> |
  <a href="README.es.md">Español</a> |
  <a href="README.pt-BR.md">Português</a> |
  <a href="README.tr.md">Türkçe</a> |
  <a href="README.ru.md">Русский</a> |
  <a href="README.uk.md">Українська</a>
</p>

LumiControl 是一套 Windows 環境光亮度控制工具。ESP32-C3 搭配
GY-302/BH1750 感測器持續提供照度資料，低資源占用的背景 Agent 依照你設定的
校準曲線，透過 DDC/CI 平順調整螢幕亮度。選用繼電器韌體時，也能控制桌面或
螢幕背後的低壓燈條。

## 下載

請至 **[GitHub Releases](https://github.com/TnzGit/lumicontrol/releases)**
下載最新的 Windows x64 安裝程式。

> [!WARNING]
> 目前的 Preview 安裝程式尚未進行程式碼簽章，Windows SmartScreen 可能會顯示
> 「未知發行者」警告。執行前請核對 Release 頁面所附的 SHA-256 校驗值。

### 使用需求

- Windows 10 或 Windows 11，x64
- 在顯示器選單中啟用 DDC/CI
- ESP32-C3 SuperMini 與 GY-302/BH1750 環境光感測器
- 如需燈條控制，可加裝支援的 5 V 繼電器模組

安裝後連接 USB 感測器並開啟 LumiControl。軟體會自動發現支援的硬體；在
「Calibration」頁面可依房間與顯示器調整照度到亮度的映射曲線。

## 主要功能

- 常駐且低資源占用的 Windows Agent，圖形介面只在需要時開啟
- 自動識別感測器版與感測器加繼電器版硬體
- 可在過渡途中重新指定目標的平順 DDC/CI 亮度變化
- 可拖曳的照度校準曲線，並保留三步還原記錄
- 偵測手動亮度調整，支援多顯示器獨立校準
- 依優先順序執行燈條規則，條件包含時間、日出、日落、天氣、照度與螢幕亮度
- 支援 NO/NC 接點映射、手動燈條控制與規則備援動作
- 淺色、深色與跟隨系統主題
- 本機診斷會遮蔽敏感硬體識別資訊

## 硬體接線

已驗證的 ESP32-C3 SuperMini 接線如下：

| 功能 | ESP32-C3 腳位 |
| --- | --- |
| GY-302/BH1750 SDA | GPIO4 |
| GY-302/BH1750 SCL | GPIO5 |
| 繼電器模組輸入 | GPIO10 |

專案提供 `sensor` 與 `sensor-relay` 兩種韌體設定。繼電器設定預設使用常見的
低電位觸發 5 V 模組。請選用具備驅動與續流保護的模組，切勿由 ESP32 GPIO
直接驅動裸繼電器線圈。市電線路必須與低壓電路妥善隔離。

## 隱私與授權

Agent 與 UI 透過僅限目前 Windows 使用者存取的命名管道通訊。設定、備份、
記錄與診斷資料保存在 `%LOCALAPPDATA%\LumiControl`；只有已啟用的規則需要天氣
資料時才會提出天氣請求，不必建立帳號。

LumiControl 採用 [PolyForm Noncommercial License 1.0.0](../../LICENSE)，
可在非商業情境下檢視、使用與修改原始碼，但**不得商業使用**，也不是 OSI
定義的開放原始碼授權。第三方元件仍依各自授權條款使用。

建置命令、架構與韌體說明請參閱[英文主 README](../../README.md)與
[`docs/v2`](../v2/)。提交貢獻前請閱讀[貢獻指南](../../CONTRIBUTING.md)。
