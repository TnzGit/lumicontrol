# LumiControl

<p align="center"><strong>Brillo del monitor en Windows adaptado a la luz ambiental.</strong></p>

<p align="center">
  <a href="../../README.md">English</a> |
  <a href="README.zh-CN.md">简体中文</a> |
  <a href="README.zh-TW.md">繁體中文</a> |
  <a href="README.ja.md">日本語</a> |
  <a href="README.ko.md">한국어</a> |
  Español |
  <a href="README.pt-BR.md">Português</a> |
  <a href="README.tr.md">Türkçe</a> |
  <a href="README.ru.md">Русский</a> |
  <a href="README.uk.md">Українська</a>
</p>

LumiControl es un controlador de brillo ambiental para Windows. Un ESP32-C3 con
sensor GY-302/BH1750 proporciona lecturas de lux en tiempo real, y un Agent ligero
en segundo plano ajusta suavemente los monitores DDC/CI según tu curva de
calibración. El firmware opcional con relé también puede controlar una tira de luz
de baja tensión situada en el escritorio o detrás del monitor. También puede
funcionar sin sensor mediante el tiempo local, la altura solar, el amanecer, el
atardecer y la duración estacional del día.

## Descarga

Descarga el instalador más reciente para Windows x64 desde
**[GitHub Releases](https://github.com/TnzGit/lumicontrol/releases)**.

> [!WARNING]
> Los instaladores Preview actuales todavía no están firmados digitalmente.
> Windows SmartScreen puede mostrar un aviso de editor desconocido. Comprueba la
> suma SHA-256 publicada en la misma Release antes de ejecutar el archivo.

### Requisitos

- Windows 10 u 11, x64
- un monitor con DDC/CI activado en su menú en pantalla
- conexión y ubicación para **Weather & sun**, o un ESP32-C3 SuperMini con sensor
  GY-302/BH1750 para **USB sensor**
- un módulo de relé de 5 V compatible si se desea controlar una tira de luz

Después de instalar, conecta el sensor por USB y abre LumiControl. El hardware
compatible se detecta automáticamente; en **Calibration** puedes adaptar la curva
lux-brillo a la habitación y a cada monitor.

## Funciones principales

- Agent residente de bajo consumo y una interfaz Tauri que se abre bajo demanda
- recomendación sin hardware basada en tiempo, sol, estación y ajuste personal
- detección USB automática para perfiles con sensor o sensor y relé
- transiciones DDC/CI suaves cuyo objetivo puede cambiar durante el movimiento
- curva de calibración arrastrable con historial de tres pasos
- detección de cambios manuales y calibración independiente por monitor
- reglas de luz con prioridad basadas en hora, amanecer, puesta de sol, tiempo,
  lux y brillo del monitor
- inversión de contactos NO/NC, control manual y acciones alternativas
- temas claro, oscuro y según el sistema
- diagnósticos locales con identificadores de hardware confidenciales ocultos

## Conexiones de hardware

Cableado validado para ESP32-C3 SuperMini:

| Función | Pin ESP32-C3 |
| --- | --- |
| GY-302/BH1750 SDA | GPIO4 |
| GY-302/BH1750 SCL | GPIO5 |
| Entrada del módulo de relé | GPIO10 |

Se incluyen los perfiles de firmware `sensor` y `sensor-relay`. El segundo supone
un módulo de relé común de 5 V activo en nivel bajo. Utiliza un módulo con driver
y protección flyback; nunca conectes directamente una bobina de relé al GPIO del
ESP32. Mantén cualquier cableado de red aislado del circuito de baja tensión.

## Privacidad y licencia

El Agent y la interfaz se comunican mediante una canalización de Windows limitada
al usuario actual. La configuración, copias, registros y diagnósticos permanecen
en `%LOCALAPPDATA%\LumiControl`. Solo se consulta el tiempo cuando una regla activa
lo necesita y no se requiere una cuenta.

LumiControl publica su código para uso no comercial bajo la
[PolyForm Noncommercial License 1.0.0](../../LICENSE). **No permite uso comercial**
y no es una licencia de código abierto aprobada por la OSI. Los componentes de
terceros conservan sus propias licencias.

Consulta el [README en inglés](../../README.md) y [`docs/v2`](../v2/) para ver la
arquitectura, la compilación y la documentación del firmware.
