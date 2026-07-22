import { useCallback, useEffect, useRef, useState } from "react";
import {
  Activity,
  Cable,
  CirclePause,
  CirclePlay,
  Gauge,
  LampDesk,
  LifeBuoy,
  LoaderCircle,
  Settings,
  SlidersHorizontal,
  SunMedium,
  TriangleAlert,
  X,
} from "lucide-react";
import * as api from "./api";
import { resolveLocale, translator } from "./i18n";
import type { AgentSnapshot, SaveStatus, SettingsDocument, ViewId } from "./types";
import { IconButton } from "./components/ui";
import CalibrationView from "./views/CalibrationView";
import HardwareView from "./views/HardwareView";
import Onboarding from "./views/Onboarding";
import RulesView from "./views/RulesView";
import SettingsView from "./views/SettingsView";
import StatusView from "./views/StatusView";
import SupportView from "./views/SupportView";

const navItems: Array<{ id: ViewId; labelKey: "status" | "calibration" | "rules" | "hardware" | "settings" | "support"; icon: typeof Gauge; relay?: boolean }> = [
  { id: "status", labelKey: "status", icon: Gauge },
  { id: "calibration", labelKey: "calibration", icon: SlidersHorizontal },
  { id: "rules", labelKey: "rules", icon: LampDesk, relay: true },
  { id: "hardware", labelKey: "hardware", icon: Cable },
  { id: "settings", labelKey: "settings", icon: Settings },
  { id: "support", labelKey: "support", icon: LifeBuoy },
];

function errorText(reason: unknown): string {
  if (reason instanceof Error) return reason.message;
  return String(reason);
}

export default function App() {
  const [snapshot, setSnapshot] = useState<AgentSnapshot | null>(null);
  const [document, setDocument] = useState<SettingsDocument | null>(null);
  const [view, setView] = useState<ViewId>("status");
  const [loading, setLoading] = useState(true);
  const [connectionError, setConnectionError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [saveStatus, setSaveStatus] = useState<SaveStatus>("idle");
  const snapshotRef = useRef<AgentSnapshot | null>(null);
  const saveTail = useRef<Promise<void>>(Promise.resolve());
  const saveGeneration = useRef(0);

  const locale = resolveLocale(document?.settings.locale);
  const t = translator(locale);

  const load = useCallback(async () => {
    setLoading(true);
    setConnectionError(null);
    try {
      const [nextSnapshot, nextDocument] = await Promise.all([api.getSnapshot(), api.getSettings()]);
      snapshotRef.current = nextSnapshot;
      setSnapshot(nextSnapshot);
      setDocument(nextDocument);
    } catch (reason) {
      setConnectionError(errorText(reason));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    snapshotRef.current = snapshot;
  }, [snapshot]);

  useEffect(() => {
    if (!snapshot || !document) return;
    let cancelled = false;
    let retryDelay = 350;
    async function watch() {
      while (!cancelled) {
        try {
          const currentRevision = snapshotRef.current?.revision ?? 0;
          const next = await api.waitForSnapshot(currentRevision, 25_000);
          if (cancelled) break;
          snapshotRef.current = next;
          setSnapshot(next);
          setConnectionError(null);
          retryDelay = 350;
        } catch (reason) {
          if (cancelled) break;
          setConnectionError(errorText(reason));
          await new Promise<void>((resolve) => window.setTimeout(resolve, retryDelay));
          retryDelay = Math.min(5_000, retryDelay * 2);
        }
      }
    }
    void watch();
    return () => { cancelled = true; };
  }, [Boolean(snapshot && document)]);

  useEffect(() => {
    const theme = document?.settings.theme ?? "system";
    if (theme === "system") delete documentElement().dataset.theme;
    else documentElement().dataset.theme = theme;
    documentElement().lang = locale === "zh" ? "zh-CN" : "en";
  }, [document?.settings.theme, locale]);

  useEffect(() => {
    if (!document) return;
    void api.setWindowMode(!document.settings.onboarding_completed).catch((reason) => {
      setActionError(errorText(reason));
    });
  }, [document?.settings.onboarding_completed]);

  const saveDocument = useCallback(async (next: SettingsDocument) => {
    const generation = ++saveGeneration.current;
    const committed = structuredClone(next);
    setDocument(committed);
    setSaveStatus("saving");
    setActionError(null);
    const task = saveTail.current
      .catch(() => undefined)
      .then(() => api.saveSettings(committed));
    saveTail.current = task;
    try {
      await task;
      if (saveGeneration.current === generation) {
        setSaveStatus("saved");
        window.setTimeout(() => {
          if (saveGeneration.current === generation) setSaveStatus("idle");
        }, 1_500);
      }
    } catch (reason) {
      if (saveGeneration.current === generation) {
        setSaveStatus("error");
        setActionError(errorText(reason));
        try {
          const persisted = await api.getSettings();
          if (saveGeneration.current === generation) setDocument(persisted);
        } catch {
          // Keep the optimistic document visible when the Agent is unreachable.
        }
      }
      throw reason;
    }
  }, []);

  async function command(operation: () => Promise<void>) {
    setActionError(null);
    try {
      await operation();
    } catch (reason) {
      setActionError(errorText(reason));
    }
  }

  async function setAutomaticPaused(paused: boolean) {
    if (!snapshot || snapshot.paused === paused) return;
    await command(async () => {
      await api.setPaused(paused);
      setSnapshot((current) => current ? { ...current, paused } : current);
      setDocument((current) => current ? { ...current, settings: { ...current.settings, paused } } : current);
    });
  }

  async function togglePause() {
    if (!snapshot) return;
    await setAutomaticPaused(!snapshot.paused);
  }

  if (loading) {
    return (
      <main className="startup-state">
        <SunMedium size={25} />
        <LoaderCircle className="spin" size={19} />
        <strong>Starting LumiControl</strong>
      </main>
    );
  }

  if (!snapshot || !document) {
    return (
      <main className="startup-state error-state">
        <TriangleAlert size={26} />
        <strong>Could not connect to Lumi Agent</strong>
        <p>{connectionError}</p>
        <button className="button button-primary" type="button" onClick={() => void load()}>Try again</button>
      </main>
    );
  }

  if (!document.settings.onboarding_completed) {
    return (
      <Onboarding
        snapshot={snapshot}
        document={document}
        error={actionError ?? connectionError}
        onSave={saveDocument}
        onRefresh={() => command(api.refreshHardware)}
        onSetLight={(on) => command(() => api.setLight(on))}
      />
    );
  }

  const relayAvailable = snapshot.device.capabilities.includes("relay");

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand"><SunMedium size={19} /><strong>{t("appName")}</strong></div>
        <div className="topbar-status">
          {connectionError ? (
            <span className="connection-status status-bad"><TriangleAlert size={14} /> Reconnecting</span>
          ) : (
            <span className={`connection-status ${snapshot.paused ? "status-neutral" : "status-good"}`}>
              <Activity size={14} /> {snapshot.paused ? t("paused") : t("live")}
            </span>
          )}
          {saveStatus !== "idle" && (
            <span className={`save-state save-${saveStatus}`}>
              {saveStatus === "saving" ? t("saving") : saveStatus === "saved" ? t("saved") : t("saveFailed")}
            </span>
          )}
          <IconButton
            label={snapshot.paused ? t("resume") : t("pause")}
            icon={snapshot.paused ? <CirclePlay size={18} /> : <CirclePause size={18} />}
            onClick={() => void togglePause()}
          />
        </div>
      </header>

      <aside className="sidebar" aria-label="Primary navigation">
        {navItems.filter((item) =>
          (!item.relay || relayAvailable)
          && (item.id !== "calibration" || document.settings.control.brightness_source === "sensor")
        ).map((item) => {
          const Icon = item.icon;
          return (
            <button
              type="button"
              key={item.id}
              className={view === item.id ? "is-active" : ""}
              aria-current={view === item.id ? "page" : undefined}
              title={t(item.labelKey)}
              onClick={() => setView(item.id)}
            >
              <Icon size={17} />
              <span>{t(item.labelKey)}</span>
            </button>
          );
        })}
      </aside>

      <main className="main-content">
        {actionError && (
          <div className="global-error" role="alert">
            <TriangleAlert size={15} />
            <span>{actionError}</span>
            <button type="button" aria-label="Dismiss" onClick={() => setActionError(null)}><X size={15} /></button>
          </div>
        )}
        {view === "status" && (
          <StatusView
            snapshot={snapshot}
            onResume={() => setAutomaticPaused(false)}
            onSetLight={(on) => command(() => api.setLight(on))}
            onClearOverride={(id) => command(() => api.clearManualOverride(id))}
          />
        )}
        {view === "calibration" && <CalibrationView snapshot={snapshot} document={document} onSave={saveDocument} />}
        {view === "rules" && relayAvailable && <RulesView snapshot={snapshot} document={document} onSave={saveDocument} />}
        {view === "hardware" && (
          <HardwareView
            snapshot={snapshot}
            onRefresh={() => command(api.refreshHardware)}
            onSetLight={(on) => command(() => api.setLight(on))}
            onClearOverride={(id) => command(() => api.clearManualOverride(id))}
          />
        )}
        {view === "settings" && <SettingsView document={document} relayAvailable={relayAvailable} onSave={saveDocument} />}
        {view === "support" && (
          <SupportView
            snapshot={snapshot}
            onExport={api.exportDiagnostics}
            getUpdateStatus={api.updateChannelStatus}
            onCheckUpdate={api.checkForUpdate}
            onInstallUpdate={api.installUpdate}
          />
        )}
      </main>
    </div>
  );
}

function documentElement(): HTMLElement {
  return window.document.documentElement;
}
