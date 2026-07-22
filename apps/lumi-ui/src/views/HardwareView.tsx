import { Monitor, RefreshCw, RotateCcw, Usb, Zap } from "lucide-react";
import type { AgentSnapshot } from "../types";
import { ActionButton, InlineNotice, Section, StatusPill } from "../components/ui";

function formatDuration(milliseconds: number | null): string {
  if (milliseconds == null) return "";
  const minutes = Math.ceil(milliseconds / 60_000);
  return `${minutes} min remaining`;
}

export default function HardwareView({
  snapshot,
  onRefresh,
  onSetLight,
  onClearOverride,
}: {
  snapshot: AgentSnapshot;
  onRefresh: () => Promise<void>;
  onSetLight: (on: boolean) => Promise<void>;
  onClearOverride: (monitorId?: string) => Promise<void>;
}) {
  const incompatibleFirmware = snapshot.device.last_error?.includes("protocol range") ?? false;
  return (
    <div className="view hardware-view">
      <div className="view-heading">
        <div>
          <h1>Hardware</h1>
          <p>Device identity, monitor compatibility, and relay verification.</p>
        </div>
        <ActionButton icon={<RefreshCw size={16} />} onClick={() => void onRefresh()}>Refresh</ActionButton>
      </div>

      <Section
        title="Lumi device"
        action={<StatusPill tone={snapshot.device.state === "connected" ? "good" : "bad"}>{snapshot.device.state.replace("_", " ")}</StatusPill>}
      >
        <div className="hardware-summary">
          <Usb size={22} />
          <div><span>Product</span><strong>{snapshot.device.product_id ?? "Not detected"}</strong></div>
          <div><span>Serial</span><strong>{snapshot.device.serial_number ?? "--"}</strong></div>
          <div><span>Hardware</span><strong>{snapshot.device.hardware_version ?? "--"}</strong></div>
          <div><span>Firmware</span><strong>{snapshot.device.firmware_version ?? "--"}</strong></div>
          <div><span>Bootloader</span><strong>{snapshot.device.bootloader_version ?? "--"}</strong></div>
          <div><span>Protocol</span><strong>{snapshot.device.negotiated_protocol == null ? "--" : `v${snapshot.device.negotiated_protocol}`}</strong></div>
          <div><span>Port</span><strong>{snapshot.device.port_name ?? "--"}</strong></div>
        </div>
        {incompatibleFirmware && (
          <InlineNotice tone="bad">Firmware update required. Install the Protocol V2 firmware, reconnect the device, then refresh hardware.</InlineNotice>
        )}
        {snapshot.device.last_error && <InlineNotice tone="bad">{snapshot.device.last_error}</InlineNotice>}
        <div className="capability-line">
          {snapshot.device.capabilities.map((capability) => <span key={capability}>{capability === "ambient_lux" ? "Ambient sensor" : "Relay"}</span>)}
        </div>
      </Section>

      <Section title={`Monitors (${snapshot.monitors.length})`}>
        <div className="hardware-list">
          {snapshot.monitors.map((monitor) => (
            <div className="hardware-row" key={monitor.id}>
              <Monitor size={19} />
              <div className="hardware-row-copy">
                <strong>{monitor.display_name}</strong>
                <small>{monitor.display_path} · {monitor.id}</small>
                {monitor.last_error && <small className="text-bad">{monitor.last_error}</small>}
              </div>
              <div className="hardware-row-state">
                <strong>{monitor.current_percent == null ? "--" : `${monitor.current_percent}%`}</strong>
                <StatusPill tone={monitor.qualified ? "good" : "bad"}>{monitor.qualified ? "Qualified" : "Unsupported"}</StatusPill>
              </div>
              {monitor.manual_override_remaining_ms != null && (
                <ActionButton
                  variant="ghost"
                  icon={<RotateCcw size={15} />}
                  title={formatDuration(monitor.manual_override_remaining_ms)}
                  onClick={() => void onClearOverride(monitor.id)}
                >
                  Auto
                </ActionButton>
              )}
            </div>
          ))}
          {snapshot.monitors.length === 0 && <InlineNotice>No DDC/CI monitors detected.</InlineNotice>}
        </div>
      </Section>

      {snapshot.relay.available && (
        <Section title="Relay verification" action={<Zap size={17} />}>
          <div className="relay-verification">
            <div>
              <span>Observed light state</span>
              <strong>{snapshot.relay.light_on ? "On" : "Off"}</strong>
            </div>
            <div>
              <span>Relay coil</span>
              <strong>{snapshot.relay.energized ? "Energized" : "Released"}</strong>
            </div>
            <div className="button-row">
              <ActionButton variant="primary" onClick={() => void onSetLight(true)}>Turn on</ActionButton>
              <ActionButton onClick={() => void onSetLight(false)}>Turn off</ActionButton>
            </div>
          </div>
          {snapshot.relay.last_error && <InlineNotice tone="bad">{snapshot.relay.last_error}</InlineNotice>}
        </Section>
      )}
    </div>
  );
}
