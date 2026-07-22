import { Download, HeartPulse, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import type { AgentSnapshot, UpdateChannelStatus, UpdateMetadata } from "../types";
import { ActionButton, InlineNotice, Section, StatusPill } from "../components/ui";

function bytes(value: number | null): string {
  if (value == null) return "--";
  return `${(value / 1024 / 1024).toFixed(1)} MB`;
}

function uptime(seconds: number): string {
  const hours = Math.floor(seconds / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  return hours ? `${hours}h ${minutes}m` : `${minutes}m`;
}

export default function SupportView({
  snapshot,
  onExport,
  getUpdateStatus,
  onCheckUpdate,
  onInstallUpdate,
}: {
  snapshot: AgentSnapshot;
  onExport: () => Promise<string>;
  getUpdateStatus: () => Promise<UpdateChannelStatus>;
  onCheckUpdate: () => Promise<UpdateMetadata | null>;
  onInstallUpdate: () => Promise<void>;
}) {
  const [exported, setExported] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [updateStatus, setUpdateStatus] = useState<UpdateChannelStatus | null>(null);
  const [availableUpdate, setAvailableUpdate] = useState<UpdateMetadata | null>(null);
  const [updateMessage, setUpdateMessage] = useState<string | null>(null);
  const [checkingUpdate, setCheckingUpdate] = useState(false);
  const [installingUpdate, setInstallingUpdate] = useState(false);

  useEffect(() => {
    void getUpdateStatus().then(setUpdateStatus).catch((reason) => setError(String(reason)));
  }, [getUpdateStatus]);

  async function exportBundle() {
    setError(null);
    setExporting(true);
    try {
      setExported(await onExport());
    } catch (reason) {
      setError(String(reason));
    } finally {
      setExporting(false);
    }
  }

  async function checkUpdate() {
    setError(null);
    setUpdateMessage(null);
    setCheckingUpdate(true);
    try {
      const update = await onCheckUpdate();
      setAvailableUpdate(update);
      if (!update) setUpdateMessage("LumiControl is up to date.");
    } catch (reason) {
      setError(String(reason));
    } finally {
      setCheckingUpdate(false);
    }
  }

  async function installUpdate() {
    setError(null);
    setInstallingUpdate(true);
    try {
      await onInstallUpdate();
      setUpdateMessage("Update installed. Restart LumiControl to finish.");
    } catch (reason) {
      setError(String(reason));
      setInstallingUpdate(false);
    }
  }

  return (
    <div className="view support-view">
      <div className="view-heading">
        <div><h1>Support</h1><p>Runtime health and a privacy-conscious diagnostic bundle.</p></div>
      </div>

      <Section title="System health" action={<StatusPill tone={snapshot.health === "healthy" ? "good" : snapshot.health === "fault" ? "bad" : "warning"}>{snapshot.health}</StatusPill>}>
        <div className="support-health">
          <HeartPulse size={23} />
          <strong>{snapshot.status_message}</strong>
        </div>
        <dl className="data-grid health-data-grid">
          <div><dt>Agent PID</dt><dd>{snapshot.resources.process_id}</dd></div>
          <div><dt>Uptime</dt><dd>{uptime(snapshot.resources.uptime_seconds)}</dd></div>
          <div><dt>CPU</dt><dd>{snapshot.resources.cpu_usage_basis_points == null ? "--" : `${(snapshot.resources.cpu_usage_basis_points / 100).toFixed(2)}%`}</dd></div>
          <div><dt>Working set</dt><dd>{bytes(snapshot.resources.working_set_bytes)}</dd></div>
          <div><dt>Threads</dt><dd>{snapshot.resources.thread_count ?? "--"}</dd></div>
          <div><dt>Handles</dt><dd>{snapshot.resources.handle_count ?? "--"}</dd></div>
          <div><dt>Reconnects</dt><dd>{snapshot.device.reconnect_count}</dd></div>
          <div><dt>Sequence gaps</dt><dd>{snapshot.sensor.sequence_gaps}</dd></div>
          <div><dt>Malformed frames</dt><dd>{snapshot.sensor.malformed_frames}</dd></div>
        </dl>
      </Section>

      <Section title="Diagnostic bundle">
        <p className="section-copy">Recent event logs, sanitized settings, hardware summary, and current health.</p>
        <ActionButton variant="primary" icon={<Download size={16} />} disabled={exporting} onClick={() => void exportBundle()}>{exporting ? "Exporting..." : "Export diagnostics"}</ActionButton>
        {exported && <InlineNotice tone="good">Saved to {exported}</InlineNotice>}
      </Section>

      <Section title="Updates">
        <div className="settings-row">
          <div>
            <strong>Version {updateStatus?.currentVersion ?? "0.2.0"}</strong>
            <small>{updateStatus?.configured ? "Signed stable channel" : "Update channel not configured in this build"}</small>
          </div>
          <ActionButton
            icon={<RefreshCw size={15} />}
            disabled={!updateStatus?.configured || checkingUpdate || installingUpdate}
            onClick={() => void checkUpdate()}
          >
            {checkingUpdate ? "Checking..." : "Check"}
          </ActionButton>
        </div>
        {availableUpdate && (
          <div className="update-available">
            <div><strong>Version {availableUpdate.version} is available</strong>{availableUpdate.notes && <p>{availableUpdate.notes}</p>}</div>
            <ActionButton variant="primary" icon={<Download size={15} />} disabled={installingUpdate} onClick={() => void installUpdate()}>{installingUpdate ? "Installing..." : "Install"}</ActionButton>
          </div>
        )}
        {updateMessage && <InlineNotice tone="good">{updateMessage}</InlineNotice>}
      </Section>

      {error && <InlineNotice tone="bad">{error}</InlineNotice>}

      <Section title="Versions">
        <dl className="data-grid compact-data-grid">
          <div><dt>Desktop app</dt><dd>{updateStatus?.currentVersion ?? "--"}</dd></div>
          <div><dt>IPC API</dt><dd>v{snapshot.api_version}</dd></div>
          <div><dt>Firmware</dt><dd>{snapshot.device.firmware_version ?? "--"}</dd></div>
          <div><dt>Device serial</dt><dd>{snapshot.device.serial_number ?? "--"}</dd></div>
        </dl>
      </Section>
    </div>
  );
}
