# LumiControl

<p align="center"><strong>Brilho do monitor no Windows adaptado à luz ambiente.</strong></p>

<p align="center">
  <a href="../../README.md">English</a> |
  <a href="README.zh-CN.md">简体中文</a> |
  <a href="README.zh-TW.md">繁體中文</a> |
  <a href="README.ja.md">日本語</a> |
  <a href="README.ko.md">한국어</a> |
  <a href="README.es.md">Español</a> |
  Português |
  <a href="README.tr.md">Türkçe</a> |
  <a href="README.ru.md">Русский</a> |
  <a href="README.uk.md">Українська</a>
</p>

LumiControl é um controlador de brilho por luz ambiente para Windows. Um ESP32-C3
com sensor GY-302/BH1750 fornece leituras de lux em tempo real, enquanto um Agent
leve em segundo plano ajusta monitores DDC/CI suavemente conforme a sua curva de
calibração. O firmware opcional com relé também pode controlar uma fita de luz de
baixa tensão na mesa ou atrás do monitor.

## Download

Baixe o instalador mais recente para Windows x64 em
**[GitHub Releases](https://github.com/TnzGit/lumicontrol/releases)**.

> [!WARNING]
> Os instaladores Preview atuais ainda não possuem assinatura de código. O Windows
> SmartScreen pode exibir um aviso de fornecedor desconhecido. Confira o checksum
> SHA-256 publicado na mesma Release antes de executar o arquivo.

### Requisitos

- Windows 10 ou Windows 11, x64
- monitor com DDC/CI ativado no menu da tela
- ESP32-C3 SuperMini com sensor de luz ambiente GY-302/BH1750
- módulo de relé de 5 V compatível, caso queira controlar uma fita de luz

Após instalar, conecte o sensor por USB e abra o LumiControl. O hardware compatível
é detectado automaticamente; em **Calibration**, ajuste a curva de lux para brilho
de acordo com o ambiente e cada monitor.

## Principais recursos

- Agent residente de baixo consumo e interface Tauri aberta somente quando preciso
- descoberta USB automática para perfis com sensor ou sensor e relé
- transições DDC/CI suaves, com mudança de alvo durante o movimento
- curva de calibração arrastável e histórico de três etapas
- detecção de alteração manual e calibração individual por monitor
- regras de iluminação por prioridade usando hora, nascer e pôr do sol, clima,
  lux e brilho do monitor
- inversão de contatos NO/NC, controle manual e ações de fallback
- temas claro, escuro e conforme o sistema
- diagnósticos locais com identificadores sensíveis de hardware ocultos

## Ligações de hardware

Fiação validada para o ESP32-C3 SuperMini:

| Função | Pino ESP32-C3 |
| --- | --- |
| GY-302/BH1750 SDA | GPIO4 |
| GY-302/BH1750 SCL | GPIO5 |
| Entrada do módulo de relé | GPIO10 |

O projeto inclui os perfis de firmware `sensor` e `sensor-relay`. O perfil com
relé considera um módulo comum de 5 V ativo em nível baixo. Use um módulo com
driver e proteção flyback; nunca acione uma bobina de relé diretamente pelo GPIO
do ESP32. Mantenha qualquer ligação à rede elétrica isolada do circuito de baixa
tensão.

## Privacidade e licença

O Agent e a interface se comunicam por um named pipe do Windows restrito ao usuário
atual. Configurações, backups, logs e diagnósticos ficam em
`%LOCALAPPDATA%\LumiControl`. Dados meteorológicos só são consultados quando uma
regra ativa precisa deles e nenhuma conta é necessária.

O código do LumiControl é disponibilizado para uso não comercial sob a
[PolyForm Noncommercial License 1.0.0](../../LICENSE). **O uso comercial não é
permitido**, e esta não é uma licença open source aprovada pela OSI. Componentes de
terceiros mantêm suas próprias licenças.

Consulte o [README em inglês](../../README.md) e [`docs/v2`](../v2/) para detalhes
de arquitetura, compilação e firmware.
