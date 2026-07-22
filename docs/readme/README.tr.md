# LumiControl

<p align="center"><strong>Windows monitör parlaklığını ortam ışığına göre otomatik ayarlar.</strong></p>

<p align="center">
  <a href="../../README.md">English</a> |
  <a href="README.zh-CN.md">简体中文</a> |
  <a href="README.zh-TW.md">繁體中文</a> |
  <a href="README.ja.md">日本語</a> |
  <a href="README.ko.md">한국어</a> |
  <a href="README.es.md">Español</a> |
  <a href="README.pt-BR.md">Português</a> |
  Türkçe |
  <a href="README.ru.md">Русский</a> |
  <a href="README.uk.md">Українська</a>
</p>

LumiControl, Windows için ortam ışığına duyarlı bir monitör parlaklık
denetleyicisidir. GY-302/BH1750 sensörlü bir ESP32-C3 anlık lux ölçümleri sağlar;
düşük kaynak kullanan arka plan Agent'ı ise kalibrasyon eğrinize göre DDC/CI
monitörlerini yumuşak biçimde ayarlar. İsteğe bağlı röle yazılımı, masa veya
monitör arkasındaki düşük voltajlı bir ışık şeridini de kontrol edebilir. Sensör
olmadan da yerel hava, güneş yüksekliği, gün doğumu, gün batımı ve mevsimsel gün
uzunluğundan önerilen parlaklığı hesaplayabilir.

## İndirme

En güncel Windows x64 kurulum dosyasını
**[GitHub Releases](https://github.com/TnzGit/lumicontrol/releases)** sayfasından
indirin.

> [!WARNING]
> Mevcut Preview kurulum dosyaları henüz kod imzalı değildir. Windows SmartScreen
> bilinmeyen yayıncı uyarısı gösterebilir. Çalıştırmadan önce aynı Release içinde
> yayımlanan SHA-256 sağlama toplamını doğrulayın.

### Gereksinimler

- Windows 10 veya Windows 11, x64
- ekran menüsünde DDC/CI etkinleştirilmiş bir monitör
- **Weather & sun** için internet ve konum ya da **USB sensor** için
  GY-302/BH1750 sensörlü ESP32-C3 SuperMini
- ışık şeridi kontrolü için isteğe bağlı, desteklenen 5 V röle modülü

Kurulumdan sonra USB sensörünü bağlayıp LumiControl'ü açın. Desteklenen donanım
otomatik bulunur; **Calibration** bölümünde oda ve monitörlerinize uygun lux-parlaklık
eğrisini ayarlayabilirsiniz.

## Öne çıkan özellikler

- sürekli çalışan düşük kaynaklı Windows Agent'ı ve isteğe bağlı Tauri arayüzü
- hava, güneş, mevsim ve kişisel ofsete dayalı donanımsız parlaklık önerisi
- yalnız sensör ve sensör+röle profilleri için otomatik USB keşfi
- hareket sırasında hedefi değiştirilebilen yumuşak DDC/CI parlaklık geçişleri
- sürüklenebilir kalibrasyon eğrisi ve üç adımlı geri alma geçmişi
- elle yapılan parlaklık değişikliklerini algılama ve monitör başına kalibrasyon
- saat, gün doğumu, gün batımı, hava durumu, lux ve monitör parlaklığına dayalı
  öncelikli ışık kuralları
- NO/NC kontak tersleme, elle ışık kontrolü ve yedek eylemler
- açık, koyu ve sistem temasını izleyen görünüm
- hassas donanım kimliklerini gizleyen yerel tanılama

## Donanım bağlantıları

Doğrulanmış ESP32-C3 SuperMini bağlantıları:

| İşlev | ESP32-C3 pini |
| --- | --- |
| GY-302/BH1750 SDA | GPIO4 |
| GY-302/BH1750 SCL | GPIO5 |
| Röle modülü girişi | GPIO10 |

`sensor` ve `sensor-relay` adlı iki yazılım profili sağlanır. Röle profili yaygın,
aktif düşük seviyeli 5 V röle modülünü varsayar. Sürücü ve geri tepme koruması olan
bir modül kullanın; çıplak röle bobinini doğrudan ESP32 GPIO'sundan sürmeyin. Şebeke
gerilimi bağlantılarını düşük voltaj devresinden yalıtın.

## Gizlilik ve lisans

Agent ile arayüz, yalnızca geçerli Windows kullanıcısının erişebildiği adlandırılmış
bir kanal üzerinden iletişim kurar. Ayarlar, yedekler, günlükler ve tanılama verileri
`%LOCALAPPDATA%\LumiControl` altında kalır. Hava durumu yalnız etkin bir kural ihtiyaç
duyduğunda sorgulanır ve hesap gerekmez.

LumiControl kaynak kodu, [PolyForm Noncommercial License 1.0.0](../../LICENSE)
kapsamında ticari olmayan kullanıma açıktır. **Ticari kullanıma izin verilmez** ve bu,
OSI onaylı bir açık kaynak lisansı değildir. Üçüncü taraf bileşenlerin kendi
lisansları geçerlidir.

Derleme, mimari ve yazılım ayrıntıları için [İngilizce README](../../README.md) ve
[`docs/v2`](../v2/) belgelerine bakın.
