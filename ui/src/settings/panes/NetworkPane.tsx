import { useState } from "react";
import { t } from "../../shared/i18n";
import { updateSettings, type AppSettings, type RefreshInterval } from "../../shared/settings";
import { Segmented, Select, TextField } from "../controls";
import { Group, Row } from "../Group";

const fixedMinutes = [2, 5, 10, 15, 30];

/** Refresh cadence and the optional proxy. */
export function NetworkPane({ settings }: { settings: AppSettings }) {
  const interval = settings.refreshInterval;
  const intervalValue = interval.type === "fixed" ? interval.minutes : 0;
  const intervalOptions = [
    { value: 0, label: t("Automatic") },
    ...fixedMinutes.map((m) => ({ value: m, label: t("%@ minutes", m) })),
  ];
  const setInterval = (value: number) => {
    const refreshInterval: RefreshInterval = value === 0 ? { type: "adaptive" } : { type: "fixed", minutes: value };
    updateSettings({ refreshInterval });
  };

  return (
    <div className="pane-stack">
      <Group title={t("Refresh")}>
        <Row title={t("Check every")} subtitle={t("How often to fetch new figures.")}>
          <Select label={t("Check every")} value={intervalValue} options={intervalOptions} onChange={setInterval} />
        </Row>
      </Group>
      <Group title={t("Network")}>
        <ProxyRows settings={settings} />
      </Group>
    </div>
  );
}

/** Host and port are saved together, and only when both are valid; invalid text stays and says why. */
function ProxyRows({ settings }: { settings: AppSettings }) {
  const proxy = settings.networkProxy;
  const [manual, setManual] = useState(proxy.enabled);
  const [host, setHost] = useState(proxy.host);
  const [port, setPort] = useState(proxy.port ? String(proxy.port) : "");
  const [hostInvalid, setHostInvalid] = useState(false);
  const [portInvalid, setPortInvalid] = useState(false);
  const scheme = proxy.scheme === "socks5" ? "socks5" : "http";

  const save = (nextHost: string, nextPort: string, nextScheme = scheme) => {
    const cleanHost = nextHost.trim();
    const portNumber = /^\d+$/.test(nextPort.trim()) ? Number(nextPort.trim()) : NaN;
    const validPort = portNumber >= 1 && portNumber <= 65535;
    setHostInvalid(cleanHost === "");
    setPortInvalid(!validPort);
    if (cleanHost === "" || !validPort) return;
    setHost(cleanHost);
    updateSettings({ networkProxy: { enabled: true, scheme: nextScheme, host: cleanHost, port: portNumber } });
  };

  const chooseMode = (mode: "system" | "manual") => {
    setManual(mode === "manual");
    if (mode === "system") {
      setHostInvalid(false);
      setPortInvalid(false);
      updateSettings({ networkProxy: { ...proxy, enabled: false } });
    } else if (proxy.host && proxy.port) {
      updateSettings({ networkProxy: { ...proxy, enabled: true } });
    }
  };

  return (
    <>
      <Row title={t("Proxy")} subtitle={t("Use macOS settings or a proxy only for Pulse.")}>
        <Select
          label={t("Proxy")}
          value={manual ? "manual" : "system"}
          options={[{ value: "system", label: t("Follow System") }, { value: "manual", label: t("Manual") }]}
          onChange={chooseMode}
        />
      </Row>
      {manual && (
        <>
          <Row title={t("Type")}>
            <Segmented
              label={t("Type")}
              value={scheme}
              options={[{ value: "http", label: t("HTTP") }, { value: "socks5", label: t("SOCKS5") }]}
              onChange={(next) => {
                if (host.trim() && port.trim()) save(host, port, next);
                else updateSettings({ networkProxy: { ...proxy, scheme: next } });
              }}
            />
          </Row>
          <Row
            title={t("Host")}
            subtitle={hostInvalid ? t("Enter a host.") : t("The proxy server's name or address.")}
            invalid={hostInvalid}
          >
            <TextField label={t("Host")} value={host} placeholder="127.0.0.1" onDraft={setHost} onCommit={(v) => save(v, port)} />
          </Row>
          <Row
            title={t("Port")}
            subtitle={portInvalid ? t("Enter a whole number from 1 to 65535.") : t("A number from 1 to 65535.")}
            invalid={portInvalid}
          >
            <TextField label={t("Port")} value={port} placeholder="7897" onDraft={setPort} onCommit={(v) => save(host, v)} />
          </Row>
        </>
      )}
    </>
  );
}
