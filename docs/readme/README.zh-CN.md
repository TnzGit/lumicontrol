# LumiControl

<p align="center"><strong>根据环境光自动调节 Windows 显示器亮度。</strong></p>

<p align="center">
  <a href="../../README.md">English</a> |
  简体中文 |
  <a href="README.zh-TW.md">繁體中文</a> |
  <a href="README.ja.md">日本語</a> |
  <a href="README.ko.md">한국어</a> |
  <a href="README.es.md">Español</a> |
  <a href="README.pt-BR.md">Português</a> |
  <a href="README.tr.md">Türkçe</a> |
  <a href="README.ru.md">Русский</a> |
  <a href="README.uk.md">Українська</a>
</p>

LumiControl 是一款 Windows 环境光亮度控制器。ESP32-C3 与
GY-302/BH1750 传感器持续提供照度读数，低资源占用的后台 Agent 根据你
绘制的校准曲线，通过 DDC/CI 平滑调节显示器亮度。可选的继电器固件还可
控制桌面或显示器背后的低压灯条。

## 下载

从 **[GitHub Releases](https://github.com/TnzGit/lumicontrol/releases)**
下载最新的 Windows x64 安装包。

> [!WARNING]
> 当前 Preview 安装包尚未进行代码签名，因此 Windows SmartScreen 可能显示
> “未知发布者”警告。运行前请核对 Release 页面同时提供的 SHA-256 校验值。

### 使用条件

- Windows 10 或 Windows 11，x64
- 在显示器菜单中启用 DDC/CI
- ESP32-C3 SuperMini 与 GY-302/BH1750 环境光传感器
- 如需控制灯条，可增加受支持的 5 V 继电器模块

安装后连接 USB 传感器并打开 LumiControl。软件会自动发现受支持的硬件；
进入“Calibration”即可根据房间和显示器调整照度到亮度的映射曲线。

## 主要功能

- 常驻、低资源占用的 Windows Agent，GUI 仅在需要时打开
- 自动识别纯传感器与传感器加继电器两种硬件配置
- 可随时重定向目标的平滑 DDC/CI 亮度过渡
- 支持拖动的照度校准曲线，并保留三步撤销历史
- 识别手动亮度调整，支持多显示器独立校准
- 按优先级执行灯条规则，条件包括时间、日出、日落、天气、照度和屏幕亮度
- 支持 NO/NC 触点映射、手动灯条控制和规则回退动作
- 浅色、深色和跟随系统三种主题
- 本地诊断会隐藏敏感硬件标识

## 硬件接线

已经验证的 ESP32-C3 SuperMini 接线如下：

| 功能 | ESP32-C3 引脚 |
| --- | --- |
| GY-302/BH1750 SDA | GPIO4 |
| GY-302/BH1750 SCL | GPIO5 |
| 继电器模块输入 | GPIO10 |

仓库提供 `sensor` 和 `sensor-relay` 两种固件配置。继电器配置默认使用常见的
低电平触发 5 V 模块。请使用带驱动和续流保护的继电器模块，绝不要用 ESP32
GPIO 直接驱动裸继电器线圈。市电部分必须与低压电路隔离。

## 隐私与许可证

Agent 与 UI 通过仅限当前 Windows 用户访问的命名管道通信。设置、备份、日志
和诊断保存在 `%LOCALAPPDATA%\LumiControl`；只有启用的规则需要天气信息时才会
发起天气请求，无需注册账号。

LumiControl 采用 [PolyForm Noncommercial License 1.0.0](../../LICENSE)，
允许非商业用途下查看、使用和修改源码，但**不允许商业使用**，也不属于 OSI
定义的开源许可证。第三方组件继续适用各自的许可证。

构建命令、架构和固件说明请查看[英文主 README](../../README.md)与
[`docs/v2`](../v2/)。参与贡献前请阅读[贡献指南](../../CONTRIBUTING.md)。
