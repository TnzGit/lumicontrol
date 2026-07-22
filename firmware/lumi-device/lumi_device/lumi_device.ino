#include <Arduino.h>
#include <ArduinoJson.h>
#include <Preferences.h>
#include <Wire.h>
#include "esp_chip_info.h"
#include "esp_mac.h"
#include "esp_system.h"
#include "esp_task_wdt.h"

#ifndef LUMI_PROFILE_RELAY
#define LUMI_PROFILE_RELAY 1
#endif

#ifndef LUMI_HARDWARE_VERSION
#define LUMI_HARDWARE_VERSION "dev-c3-1"
#endif

#define LUMI_FIRMWARE_VERSION "2.0.0"
#define LUMI_BOOTLOADER_VERSION "1.0.0"

#if ARDUINOJSON_VERSION_MAJOR != 7 || ARDUINOJSON_VERSION_MINOR != 4 || ARDUINOJSON_VERSION_REVISION != 3
#error "Lumi firmware requires the pinned ArduinoJson 7.4.3 release"
#endif

static constexpr uint16_t PROTOCOL_VERSION = 2;
static constexpr size_t MAX_FRAME_BYTES = 1024;
static constexpr uint32_t SERIAL_BAUD = 115200;
static constexpr int SDA_PIN = 4;
static constexpr int SCL_PIN = 5;
static constexpr uint8_t BH1750_ADDRESS = 0x23;
static constexpr int RELAY_PIN = 10;
static constexpr bool RELAY_ACTIVE_LOW = true;
static constexpr uint32_t SENSOR_CONVERSION_MS = 180;
static constexpr uint32_t WATCHDOG_TIMEOUT_MS = 5000;
static constexpr uint32_t SERIAL_DISCONNECT_GRACE_MS = 750;

static Preferences factoryPreferences;
static JsonDocument inputDocument;
static JsonDocument outputDocument;
static char serialNumber[32] = {0};
static char resetReason[24] = {0};
static char inputFrame[MAX_FRAME_BYTES + 1] = {0};
static size_t inputLength = 0;
static bool droppingOversizedFrame = false;
static uint32_t malformedFrames = 0;
static uint32_t eventSequence = 0;
static bool streamEnabled = false;
static uint32_t streamIntervalMs = 1000;
static uint16_t includeStatusEvery = 30;
static uint32_t samplesSinceStatus = 0;
static bool relayEnergized = false;
static bool lastSensorHealthy = false;
static bool hasLastLux = false;
static float lastLux = 0.0f;
static uint32_t lastValidSampleAt = 0;
static bool rebootPending = false;
static uint32_t rebootAt = 0;
static bool serialDisconnectTiming = false;
static uint32_t serialDisconnectedAt = 0;
static char outputFrame[MAX_FRAME_BYTES + 1] = {0};

enum class SensorPhase : uint8_t {
  Idle,
  Converting,
};

static SensorPhase sensorPhase = SensorPhase::Idle;
static uint32_t conversionStartedAt = 0;
static uint32_t conversionReadyAt = 0;
static uint32_t nextConversionAt = 0;

static bool deadlineReached(uint32_t now, uint32_t deadline) {
  return static_cast<int32_t>(now - deadline) >= 0;
}

static const char *productId() {
#if LUMI_PROFILE_RELAY
  return "lumi-sensor-relay";
#else
  return "lumi-sensor";
#endif
}

static const char *resetReasonName(esp_reset_reason_t reason) {
  switch (reason) {
    case ESP_RST_POWERON:
      return "power_on";
    case ESP_RST_SW:
      return "software";
    case ESP_RST_PANIC:
      return "panic";
    case ESP_RST_INT_WDT:
    case ESP_RST_TASK_WDT:
    case ESP_RST_WDT:
      return "watchdog";
    case ESP_RST_DEEPSLEEP:
      return "deep_sleep";
    case ESP_RST_BROWNOUT:
      return "brownout";
    default:
      return "unknown";
  }
}

static void loadIdentity() {
  strlcpy(resetReason, resetReasonName(esp_reset_reason()), sizeof(resetReason));
  factoryPreferences.begin("lumi_factory", true);
  String factorySerial = factoryPreferences.getString("serial", "");
  factoryPreferences.end();
  factorySerial.trim();
  if (!factorySerial.isEmpty()) {
    factorySerial.toCharArray(serialNumber, sizeof(serialNumber));
    return;
  }

  uint8_t mac[6] = {0};
  if (esp_read_mac(mac, ESP_MAC_BASE) == ESP_OK) {
    snprintf(
      serialNumber,
      sizeof(serialNumber),
      "DEV-%02X%02X%02X%02X%02X%02X",
      mac[0],
      mac[1],
      mac[2],
      mac[3],
      mac[4],
      mac[5]
    );
  } else {
    strlcpy(serialNumber, "DEV-UNKNOWN", sizeof(serialNumber));
  }
}

static void writeDocument(JsonDocument &document, bool telemetry = false) {
  size_t length = serializeJson(document, outputFrame, sizeof(outputFrame));
  if (length == 0 || length >= MAX_FRAME_BYTES) {
    return;
  }
  outputFrame[length++] = '\n';
  if (telemetry && Serial.availableForWrite() < static_cast<int>(length)) {
    return;
  }
  Serial.write(reinterpret_cast<const uint8_t *>(outputFrame), length);
}

static JsonObject beginSuccess(JsonDocument &document, uint32_t id) {
  document["type"] = "response";
  document["protocol"] = PROTOCOL_VERSION;
  document["id"] = id;
  document["ok"] = true;
  return document["result"].to<JsonObject>();
}

static void sendError(
  uint32_t id,
  const char *code,
  const char *message
) {
  JsonDocument &document = outputDocument;
  document.clear();
  document["type"] = "response";
  document["protocol"] = PROTOCOL_VERSION;
  document["id"] = id;
  document["ok"] = false;
  JsonObject error = document["error"].to<JsonObject>();
  error["code"] = code;
  error["message"] = message;
  writeDocument(document);
}

static void sendHello(uint32_t id) {
  JsonDocument &document = outputDocument;
  document.clear();
  JsonObject result = beginSuccess(document, id);
  result["product_id"] = productId();
  result["serial_number"] = serialNumber;
  result["hardware_version"] = LUMI_HARDWARE_VERSION;
  result["firmware_version"] = LUMI_FIRMWARE_VERSION;
  result["bootloader_version"] = LUMI_BOOTLOADER_VERSION;
  result["protocol_min"] = PROTOCOL_VERSION;
  result["protocol_max"] = PROTOCOL_VERSION;
  JsonArray capabilities = result["capabilities"].to<JsonArray>();
  capabilities.add("ambient_lux");
#if LUMI_PROFILE_RELAY
  capabilities.add("relay");
#endif
  writeDocument(document);
}

static JsonObject appendSensorStatus(JsonObject parent, uint32_t now) {
  JsonObject sensor = parent["sensor"].to<JsonObject>();
  sensor["healthy"] = lastSensorHealthy;
  if (hasLastLux) {
    sensor["lux"] = lastLux;
    sensor["sample_age_ms"] = now - lastValidSampleAt;
  } else {
    sensor["sample_age_ms"] = nullptr;
  }
  return sensor;
}

static void appendRelayStatus(JsonObject parent) {
  JsonObject relay = parent["relay"].to<JsonObject>();
#if LUMI_PROFILE_RELAY
  relay["available"] = true;
  relay["energized"] = relayEnergized;
#else
  relay["available"] = false;
#endif
}

static void appendDeviceStatus(JsonObject result, uint32_t now) {
  appendSensorStatus(result, now);
  appendRelayStatus(result);
  result["uptime_ms"] = static_cast<uint64_t>(now);
  result["reset_reason"] = resetReason;
  result["malformed_frames"] = malformedFrames;
}

static void sendStatusResponse(uint32_t id) {
  JsonDocument &document = outputDocument;
  document.clear();
  JsonObject result = beginSuccess(document, id);
  appendDeviceStatus(result, millis());
  writeDocument(document);
}

static void sendStatusEvent() {
  JsonDocument &document = outputDocument;
  document.clear();
  document["type"] = "event";
  document["protocol"] = PROTOCOL_VERSION;
  document["event"] = "device.status";
  document["seq"] = ++eventSequence;
  uint32_t now = millis();
  document["uptime_ms"] = static_cast<uint64_t>(now);
  JsonObject data = document["data"].to<JsonObject>();
  appendDeviceStatus(data, now);
  writeDocument(document, true);
}

static int relayPinLevel(bool energized) {
  if (RELAY_ACTIVE_LOW) {
    return energized ? LOW : HIGH;
  }
  return energized ? HIGH : LOW;
}

static void applyRelay(bool energized) {
#if LUMI_PROFILE_RELAY
  digitalWrite(RELAY_PIN, relayPinLevel(energized));
  relayEnergized = energized;
#else
  (void)energized;
#endif
}

static void sendRelayResult(uint32_t id) {
  JsonDocument &document = outputDocument;
  document.clear();
  JsonObject result = beginSuccess(document, id);
  result["available"] = true;
  result["energized"] = relayEnergized;
  writeDocument(document);
}

static void sendStreamResult(uint32_t id) {
  JsonDocument &document = outputDocument;
  document.clear();
  JsonObject result = beginSuccess(document, id);
  result["ambient_lux_interval_ms"] = streamIntervalMs;
  result["include_status_every"] = includeStatusEvery;
  writeDocument(document);
}

static void sendRebootResult(uint32_t id) {
  JsonDocument &document = outputDocument;
  document.clear();
  JsonObject result = beginSuccess(document, id);
  result["rebooting"] = true;
  writeDocument(document);
  Serial.flush();
  rebootPending = true;
  rebootAt = millis() + 100;
}

static bool requestHasValidEnvelope(
  JsonDocument &request,
  uint32_t &id,
  const char *&command
) {
  id = request["id"].is<uint32_t>() ? request["id"].as<uint32_t>() : 0;
  if (!request["type"].is<const char *>()
      || strcmp(request["type"].as<const char *>(), "request") != 0
      || !request["protocol"].is<uint16_t>()
      || !request["id"].is<uint32_t>()
      || !request["command"].is<const char *>()) {
    sendError(id, "invalid_request", "invalid request envelope");
    return false;
  }
  if (request["protocol"].as<uint16_t>() != PROTOCOL_VERSION) {
    sendError(id, "unsupported_protocol", "firmware supports protocol 2");
    return false;
  }
  command = request["command"].as<const char *>();
  if (command[0] == '\0') {
    sendError(id, "invalid_request", "command must not be empty");
    return false;
  }
  return true;
}

static void processRequest(char *frame, size_t length) {
  JsonDocument &request = inputDocument;
  request.clear();
  DeserializationError parsing = deserializeJson(request, frame, length);
  if (parsing) {
    ++malformedFrames;
    sendError(0, "invalid_request", "malformed JSON");
    return;
  }

  uint32_t id = 0;
  const char *command = nullptr;
  if (!requestHasValidEnvelope(request, id, command)) {
    ++malformedFrames;
    return;
  }

  JsonObject params;
  if (request["params"].is<JsonObject>()) {
    params = request["params"].as<JsonObject>();
  } else if (!request["params"].isNull()) {
    sendError(id, "invalid_request", "params must be an object");
    return;
  }

  if (strcmp(command, "device.hello") == 0) {
    sendHello(id);
  } else if (strcmp(command, "device.get_status") == 0) {
    sendStatusResponse(id);
  } else if (strcmp(command, "stream.configure") == 0) {
    if (!params["ambient_lux_interval_ms"].is<uint32_t>()
        || !params["include_status_every"].is<uint16_t>()) {
      sendError(id, "invalid_parameter", "stream parameters are required");
      return;
    }
    uint32_t interval = params["ambient_lux_interval_ms"].as<uint32_t>();
    uint16_t statusEvery = params["include_status_every"].as<uint16_t>();
    if (interval < 200 || interval > 5000 || statusEvery < 1 || statusEvery > 300) {
      sendError(id, "invalid_parameter", "stream parameters are outside allowed range");
      return;
    }
    streamIntervalMs = interval;
    includeStatusEvery = statusEvery;
    samplesSinceStatus = 0;
    streamEnabled = true;
    sensorPhase = SensorPhase::Idle;
    nextConversionAt = millis();
    sendStreamResult(id);
  } else if (strcmp(command, "relay.set") == 0) {
#if LUMI_PROFILE_RELAY
    if (!params["energized"].is<bool>()) {
      sendError(id, "invalid_parameter", "energized must be boolean");
      return;
    }
    applyRelay(params["energized"].as<bool>());
    sendRelayResult(id);
#else
    sendError(id, "unsupported_capability", "relay is not installed");
#endif
  } else if (strcmp(command, "device.reboot") == 0) {
    sendRebootResult(id);
  } else {
    sendError(id, "unsupported_command", "command is not supported");
  }
}

static void processSerialInput() {
  while (Serial.available() > 0) {
    int value = Serial.read();
    if (value < 0) {
      break;
    }
    char byte = static_cast<char>(value);
    if (byte == '\n') {
      if (droppingOversizedFrame) {
        droppingOversizedFrame = false;
        inputLength = 0;
        ++malformedFrames;
        sendError(0, "invalid_request", "frame exceeds 1024 bytes");
      } else if (inputLength > 0) {
        inputFrame[inputLength] = '\0';
        processRequest(inputFrame, inputLength);
        inputLength = 0;
      }
      continue;
    }
    if (byte == '\r' || droppingOversizedFrame) {
      continue;
    }
    if (inputLength >= MAX_FRAME_BYTES) {
      droppingOversizedFrame = true;
      inputLength = 0;
      continue;
    }
    inputFrame[inputLength++] = byte;
  }
}

static bool sendBh1750Command(uint8_t command) {
  Wire.beginTransmission(BH1750_ADDRESS);
  Wire.write(command);
  return Wire.endTransmission() == 0;
}

static bool beginSensorConversion(uint32_t now) {
  if (!sendBh1750Command(0x01) || !sendBh1750Command(0x20)) {
    return false;
  }
  conversionStartedAt = now;
  conversionReadyAt = now + SENSOR_CONVERSION_MS;
  sensorPhase = SensorPhase::Converting;
  return true;
}

static bool finishSensorConversion(float &lux, const char *&quality) {
  int count = Wire.requestFrom(static_cast<int>(BH1750_ADDRESS), 2);
  if (count != 2) {
    quality = "read_error";
    return false;
  }
  uint16_t raw = (static_cast<uint16_t>(Wire.read()) << 8) | Wire.read();
  if (raw == 0) {
    quality = "below_range";
    return false;
  }
  if (raw == UINT16_MAX) {
    quality = "saturated";
    return false;
  }
  lux = raw / 1.2f;
  quality = "valid";
  return true;
}

static void sendSensorEvent(bool valid, float lux, const char *quality, uint32_t now) {
  JsonDocument &document = outputDocument;
  document.clear();
  document["type"] = "event";
  document["protocol"] = PROTOCOL_VERSION;
  document["event"] = "sensor.sample";
  document["seq"] = ++eventSequence;
  document["uptime_ms"] = static_cast<uint64_t>(now);
  JsonObject data = document["data"].to<JsonObject>();
  if (valid) {
    data["lux"] = lux;
  }
  data["quality"] = quality;
  writeDocument(document, true);
}

static void processSensor(uint32_t now) {
  if (!streamEnabled) {
    return;
  }
  if (sensorPhase == SensorPhase::Idle && deadlineReached(now, nextConversionAt)) {
    if (!beginSensorConversion(now)) {
      lastSensorHealthy = false;
      sendSensorEvent(false, 0.0f, "read_error", now);
      ++samplesSinceStatus;
      nextConversionAt = now + streamIntervalMs;
    }
    return;
  }
  if (sensorPhase != SensorPhase::Converting || !deadlineReached(now, conversionReadyAt)) {
    return;
  }

  float lux = 0.0f;
  const char *quality = "read_error";
  bool valid = finishSensorConversion(lux, quality);
  lastSensorHealthy = strcmp(quality, "read_error") != 0;
  if (valid) {
    hasLastLux = true;
    lastLux = lux;
    lastValidSampleAt = now;
  }
  sendSensorEvent(valid, lux, quality, now);
  ++samplesSinceStatus;
  if (samplesSinceStatus >= includeStatusEvery) {
    samplesSinceStatus = 0;
    sendStatusEvent();
  }
  sensorPhase = SensorPhase::Idle;
  nextConversionAt = conversionStartedAt + streamIntervalMs;
  if (deadlineReached(now, nextConversionAt)) {
    nextConversionAt = now + 1;
  }
}

static void processSerialConnection(uint32_t now) {
  if (Serial) {
    serialDisconnectTiming = false;
    return;
  }
  if (!streamEnabled) {
    return;
  }
  if (!serialDisconnectTiming) {
    serialDisconnectTiming = true;
    serialDisconnectedAt = now;
    return;
  }
  if (deadlineReached(now, serialDisconnectedAt + SERIAL_DISCONNECT_GRACE_MS)) {
    streamEnabled = false;
    sensorPhase = SensorPhase::Idle;
    inputLength = 0;
    droppingOversizedFrame = false;
  }
}

static void initializeWatchdog() {
  esp_task_wdt_config_t configuration = {
    .timeout_ms = WATCHDOG_TIMEOUT_MS,
    .idle_core_mask = 0,
    .trigger_panic = true,
  };
  esp_err_t initialized = esp_task_wdt_init(&configuration);
  if (initialized == ESP_OK || initialized == ESP_ERR_INVALID_STATE) {
    esp_task_wdt_add(nullptr);
  }
}

void setup() {
  Serial.begin(SERIAL_BAUD);
  Serial.setTxTimeoutMs(2);
  Serial.setTxBufferSize(512);
  loadIdentity();
#if LUMI_PROFILE_RELAY
  digitalWrite(RELAY_PIN, relayPinLevel(false));
  pinMode(RELAY_PIN, OUTPUT);
  applyRelay(false);
#endif
  Wire.begin(SDA_PIN, SCL_PIN);
  Wire.setClock(100000);
  Wire.setTimeOut(50);
  initializeWatchdog();
}

void loop() {
  esp_task_wdt_reset();
  processSerialInput();
  uint32_t now = millis();
  processSerialConnection(now);
  processSensor(now);
  processSerialInput();
  if (rebootPending && deadlineReached(now, rebootAt)) {
    ESP.restart();
  }
  delay(1);
}
