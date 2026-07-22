# LumiControl

<p align="center"><strong>주변 밝기에 맞춰 Windows 모니터 밝기를 자동으로 조절합니다.</strong></p>

<p align="center">
  <a href="../../README.md">English</a> |
  <a href="README.zh-CN.md">简体中文</a> |
  <a href="README.zh-TW.md">繁體中文</a> |
  <a href="README.ja.md">日本語</a> |
  한국어 |
  <a href="README.es.md">Español</a> |
  <a href="README.pt-BR.md">Português</a> |
  <a href="README.tr.md">Türkçe</a> |
  <a href="README.ru.md">Русский</a> |
  <a href="README.uk.md">Українська</a>
</p>

LumiControl은 Windows용 주변광 기반 모니터 밝기 컨트롤러입니다. ESP32-C3와
GY-302/BH1750 센서가 실시간 조도를 제공하고, 자원 사용량이 낮은 백그라운드 Agent가
사용자 보정 곡선에 따라 DDC/CI 모니터 밝기를 부드럽게 조절합니다. 선택형 릴레이
펌웨어를 사용하면 책상이나 모니터 뒤의 저전압 조명 스트립도 제어할 수 있습니다.
센서 없이도 지역 날씨, 태양 고도, 일출과 일몰, 계절별 낮 길이로 권장 밝기를 계산합니다.

## 다운로드

최신 Windows x64 설치 파일은
**[GitHub Releases](https://github.com/TnzGit/lumicontrol/releases)** 에서 받을 수 있습니다.

> [!WARNING]
> 현재 Preview 설치 파일은 코드 서명되지 않았습니다. Windows SmartScreen에 알 수 없는
> 게시자 경고가 표시될 수 있으므로 실행 전에 Release에 함께 제공된 SHA-256 체크섬을
> 확인하세요.

### 요구 사항

- Windows 10 또는 Windows 11, x64
- 화면 메뉴에서 DDC/CI를 활성화한 모니터
- **Weather & sun**용 인터넷과 위치 정보 또는 **USB sensor**용 ESP32-C3
  SuperMini와 GY-302/BH1750 센서
- 조명 제어가 필요한 경우 지원되는 5 V 릴레이 모듈

설치 후 USB 센서를 연결하고 LumiControl을 엽니다. 지원 하드웨어는 자동으로 검색되며,
“Calibration”에서 방과 모니터에 맞는 조도-밝기 곡선을 설정할 수 있습니다.

## 주요 기능

- 항상 실행되는 저자원 Windows Agent와 필요할 때만 여는 Tauri UI
- 날씨, 태양, 계절, 개인 오프셋을 이용한 하드웨어 없는 밝기 추천
- 센서 전용 및 센서+릴레이 USB 하드웨어 자동 검색
- 이동 중에도 목표를 바꿀 수 있는 부드러운 DDC/CI 밝기 전환
- 드래그 가능한 조도 보정 곡선과 3단계 되돌리기 기록
- 수동 밝기 변경 감지와 모니터별 보정
- 시간, 일출, 일몰, 날씨, 조도, 모니터 밝기를 이용한 우선순위 조명 규칙
- NO/NC 접점 반전, 수동 조명 제어, 폴백 동작
- 라이트, 다크, 시스템 연동 테마
- 민감한 하드웨어 식별자를 가리는 로컬 진단

## 하드웨어 배선

검증된 ESP32-C3 SuperMini 연결:

| 기능 | ESP32-C3 핀 |
| --- | --- |
| GY-302/BH1750 SDA | GPIO4 |
| GY-302/BH1750 SCL | GPIO5 |
| 릴레이 모듈 입력 | GPIO10 |

`sensor`와 `sensor-relay` 두 가지 펌웨어 프로필을 제공합니다. 릴레이 프로필은 일반적인
액티브 로우 5 V 릴레이 모듈을 기준으로 합니다. 드라이버와 플라이백 보호가 포함된 모듈을
사용하고, ESP32 GPIO로 릴레이 코일을 직접 구동하지 마세요. 상용 전원 배선은 저전압
회로와 반드시 분리해야 합니다.

## 개인정보 및 라이선스

Agent와 UI는 현재 Windows 사용자만 접근할 수 있는 명명된 파이프로 통신합니다. 설정,
백업, 로그, 진단은 `%LOCALAPPDATA%\LumiControl`에 저장됩니다. 활성화된 규칙에 날씨가
필요할 때만 날씨 요청을 보내며 계정은 필요하지 않습니다.

LumiControl은 [PolyForm Noncommercial License 1.0.0](../../LICENSE)에 따라
비상업적 사용을 위해 소스를 공개합니다. **상업적 사용은 허용되지 않으며**, OSI가 정의한
오픈 소스 라이선스가 아닙니다. 타사 구성 요소에는 각각의 라이선스가 적용됩니다.

빌드 명령, 아키텍처와 펌웨어 문서는 [영문 README](../../README.md) 및
[`docs/v2`](../v2/)에서 확인할 수 있습니다.
