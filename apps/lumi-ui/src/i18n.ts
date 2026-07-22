export type Locale = "en" | "zh";

const messages = {
  en: {
    appName: "LumiControl",
    status: "Status",
    calibration: "Calibration",
    rules: "Light rules",
    hardware: "Hardware",
    settings: "Settings",
    support: "Support",
    live: "Live",
    paused: "Paused",
    pause: "Pause",
    resume: "Resume",
    runNow: "Run now",
    saving: "Saving",
    saved: "Saved",
    saveFailed: "Save failed",
  },
  zh: {
    appName: "LumiControl",
    status: "状态",
    calibration: "校准",
    rules: "灯光规则",
    hardware: "硬件",
    settings: "设置",
    support: "支持",
    live: "运行中",
    paused: "已暂停",
    pause: "暂停",
    resume: "继续",
    runNow: "立即运行",
    saving: "正在保存",
    saved: "已保存",
    saveFailed: "保存失败",
  },
} as const;

export type MessageKey = keyof (typeof messages)["en"];

export function resolveLocale(setting?: string): Locale {
  const requested = setting && setting !== "system" ? setting : navigator.language;
  return requested.toLowerCase().startsWith("zh") ? "zh" : "en";
}

export function translator(locale: Locale) {
  return (key: MessageKey) => messages[locale][key] ?? messages.en[key];
}
