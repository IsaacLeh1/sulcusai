// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { nets, type NetsView, type NetView } from "../views/groups/Networks";
import { errorText } from "../api";
import { t } from "../i18n";
import { useDismiss } from "../useDismiss";
import type { PushToast } from "./Toasts";
import type { View } from "../App";

interface NetworkStatusProps {
  toast: PushToast;
  setView: (v: View) => void;
}

/** Status bar widget showing connected virtual networks with 1-click disconnect, online peer listing, and quick jump to group chats. */
export function NetworkStatus({ toast, setView }: NetworkStatusProps) {
  const [v, setV] = useState<NetsView | null>(null);
  const [open, setOpen] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);

  useDismiss(open, () => setOpen(false), ref);

  const load = useCallback(() => {
    nets
      .view()
      .then(setV)
      .catch(() => {});
  }, []);

  useEffect(() => {
    load();
    const unlisten = listen("social:networks", load);
    // Periodic refresh keeps online peer status up to date.
    const timer = window.setInterval(load, 5000);
    return () => {
      window.clearInterval(timer);
      unlisten.then((fn) => fn());
    };
  }, [load]);

  const disconnectNetwork = async (n: NetView) => {
    setBusyId(n.id);
    try {
      const res = await nets.connect(n.id, false);
      setV(res);
      toast(t("Disconnected from {name}", { name: n.name }));
    } catch (e) {
      toast(errorText(e));
    } finally {
      setBusyId(null);
    }
  };

  const connectNetwork = async (n: NetView) => {
    setBusyId(n.id);
    try {
      const res = await nets.connect(n.id, true);
      setV(res);
      toast(t("Connected to {name}", { name: n.name }));
    } catch (e) {
      toast(errorText(e));
    } finally {
      setBusyId(null);
    }
  };

  const disconnectAll = async () => {
    const connected = v?.mine.filter((n) => n.connected) || [];
    for (const n of connected) {
      try {
        await nets.connect(n.id, false);
      } catch {
        // Continue disconnecting remaining networks even if one errors.
      }
    }
    await load();
    toast(t("Disconnected all networks."));
  };

  const jumpToGroup = (groupId: string) => {
    setOpen(false);
    setView("people");
    window.dispatchEvent(new CustomEvent("social:open-group", { detail: { groupId } }));
  };

  const goToNetworks = () => {
    setOpen(false);
    setView("people");
  };

  const connected = v?.mine.filter((n) => n.connected) || [];
  const isOnline = connected.length > 0;
  const allOnlinePeers = connected.flatMap((n) => n.members.filter((m) => m.online && !m.me));

  return (
    <div ref={ref} style={{ position: "relative", display: "inline-flex", alignItems: "center" }}>
      <button
        type="button"
        className={`status-item ${isOnline ? "active" : ""}`}
        title={
          isOnline
            ? allOnlinePeers.length > 0
              ? t("Virtual LAN active ({n} connected): {peers}", { n: connected.length, peers: allOnlinePeers.map((p) => p.name).join(", ") })
              : t("Virtual LAN active ({name}): waiting for peers", { name: connected[0]?.name || "" })
            : t("Virtual LAN status")
        }
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        style={{
          display: "inline-flex",
          alignItems: "center",
          gap: 6,
          background: "none",
          border: 0,
          cursor: "pointer",
          padding: "2px 8px",
          borderRadius: 4,
          fontSize: 12,
        }}
      >
        <span
          className={`net-dot ${isOnline ? "on" : ""}`}
          style={{
            display: "inline-block",
            width: 8,
            height: 8,
            borderRadius: "50%",
            background: isOnline ? "#23a55a" : "var(--muted, #80848e)",
            boxShadow: isOnline ? "0 0 6px rgba(35, 165, 90, 0.6)" : "none",
          }}
        />
        <span>
          {isOnline
            ? connected.length === 1
              ? allOnlinePeers.length > 0
                ? `${connected[0].name} (${allOnlinePeers.length})`
                : connected[0].name
              : t("{n} Networks", { n: connected.length })
            : v?.lan_on
            ? t("LAN Idle")
            : t("LAN Off")}
        </span>
      </button>

      {open && (
        <div
          className="conn-menu"
          style={{
            position: "absolute",
            bottom: "calc(100% + 8px)",
            left: 0,
            width: 320,
            maxHeight: 420,
            overflowY: "auto",
            zIndex: 100,
            background: "var(--social-side, var(--panel, #2b2d31))",
            border: "1px solid var(--social-sel, var(--border))",
            borderRadius: 8,
            boxShadow: "0 8px 24px rgba(0, 0, 0, 0.45)",
            padding: 12,
            display: "flex",
            flexDirection: "column",
            gap: 10,
          }}
        >
          {/* Header */}
          <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", borderBottom: "1px solid var(--border)", paddingBottom: 8 }}>
            <div>
              <strong style={{ fontSize: 13 }}>{t("Virtual Networks")}</strong>
              <div className="small muted" style={{ fontSize: 11 }}>
                {v?.lan_on ? t("IP: {ip}", { ip: v.my_address || "0.0.0.0" }) : t("Virtual LAN disabled")}
              </div>
            </div>
            {connected.length > 1 && (
              <button
                type="button"
                className="link small"
                onClick={disconnectAll}
                style={{ fontSize: 11 }}
              >
                {t("Disconnect All")}
              </button>
            )}
          </div>

          {/* Active Networks */}
          {connected.length > 0 ? (
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {connected.map((n) => {
                const onlinePeers = n.members.filter((m) => m.online && !m.me);
                const isBusy = busyId === n.id;
                return (
                  <div
                    key={n.id}
                    style={{
                      background: "rgba(0, 0, 0, 0.2)",
                      borderRadius: 6,
                      padding: 10,
                      display: "flex",
                      flexDirection: "column",
                      gap: 6,
                      border: "1px solid rgba(255, 255, 255, 0.05)",
                    }}
                  >
                    <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center" }}>
                      <span style={{ fontWeight: 600, fontSize: 13, display: "flex", alignItems: "center", gap: 6 }}>
                        <span style={{ width: 6, height: 6, borderRadius: "50%", background: "#23a55a" }} />
                        {n.name}
                      </span>
                      <button
                        type="button"
                        className="btn-discord secondary"
                        disabled={isBusy}
                        onClick={() => disconnectNetwork(n)}
                        style={{ padding: "2px 8px", fontSize: 11 }}
                      >
                        {isBusy ? t("...") : t("Disconnect")}
                      </button>
                    </div>

                    {/* Connected members list */}
                    <div style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 4 }}>
                      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", fontSize: 11, fontWeight: 700, color: "var(--muted, #80848e)", textTransform: "uppercase", letterSpacing: "0.04em" }}>
                        <span>{t("Connected Members ({n})", { n: n.members.filter((m) => m.online).length })}</span>
                        <span style={{ fontSize: 10, color: "#23a55a" }}>● {t("Active")}</span>
                      </div>
                      {n.members.filter((m) => m.online).map((m) => (
                        <div
                          key={m.peer}
                          style={{
                            display: "flex",
                            alignItems: "center",
                            justifyContent: "space-between",
                            padding: "4px 8px",
                            borderRadius: 4,
                            background: m.me ? "rgba(35, 165, 90, 0.08)" : "rgba(255, 255, 255, 0.04)",
                            border: m.me ? "1px solid rgba(35, 165, 90, 0.2)" : "1px solid rgba(255, 255, 255, 0.05)",
                          }}
                        >
                          <div style={{ display: "flex", alignItems: "center", gap: 6, minWidth: 0 }}>
                            <span
                              style={{
                                width: 7,
                                height: 7,
                                borderRadius: "50%",
                                background: "#23a55a",
                                boxShadow: "0 0 5px rgba(35, 165, 90, 0.8)",
                                flexShrink: 0,
                              }}
                            />
                            <span style={{ fontWeight: 600, fontSize: 12, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                              {m.name}
                            </span>
                            {m.me && <span className="small muted" style={{ fontSize: 10 }}>{t("(you)")}</span>}
                            {m.admin && !m.me && <span className="small" style={{ fontSize: 10, color: "var(--accent)" }}>{t("admin")}</span>}
                          </div>
                          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                            <button
                              type="button"
                              className="icon-ghost"
                              title={t("Copy IP: {ip}", { ip: m.address })}
                              onClick={() => {
                                navigator.clipboard.writeText(m.address).then(() => toast(t("Copied {ip}", { ip: m.address })));
                              }}
                              style={{ padding: "1px 4px", fontSize: 10, fontFamily: "monospace", borderRadius: 3, background: "rgba(0,0,0,0.25)" }}
                            >
                              {m.address}
                            </button>
                            {!m.me && m.ping !== null && (
                              <span
                                className={`radmin-ping-badge ${m.ping < 50 ? "fast" : m.ping < 150 ? "medium" : "slow"}`}
                                style={{ fontSize: 10, padding: "1px 4px" }}
                              >
                                {m.ping} ms
                              </span>
                            )}
                          </div>
                        </div>
                      ))}
                      {onlinePeers.length === 0 && (
                        <div className="small muted" style={{ fontSize: 11, fontStyle: "italic", padding: "4px 2px" }}>
                          {t("No other members connected right now.")}
                        </div>
                      )}
                    </div>

                    {/* Group chat link */}
                    {n.group && (
                      <button
                        type="button"
                        className="link small"
                        onClick={() => jumpToGroup(n.group!)}
                        style={{ alignSelf: "flex-start", padding: 0, fontSize: 11, color: "var(--accent)" }}
                      >
                        → {t("Open Group Chat")}
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          ) : (
            <div style={{ padding: "8px 0" }}>
              <p className="small muted" style={{ margin: "0 0 8px", fontSize: 12 }}>
                {t("Not connected to any virtual network.")}
              </p>
              {v && v.mine.length > 0 && (
                <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
                  <div className="small muted" style={{ fontSize: 11 }}>{t("Saved networks:")}</div>
                  {v.mine.map((n) => (
                    <div key={n.id} style={{ display: "flex", justifyContent: "space-between", alignItems: "center", padding: "4px 6px", borderRadius: 4, background: "rgba(0,0,0,0.15)" }}>
                      <span style={{ fontSize: 12 }}>{n.name}</span>
                      <button
                        type="button"
                        className="btn-discord"
                        disabled={busyId === n.id}
                        onClick={() => connectNetwork(n)}
                        style={{ padding: "2px 8px", fontSize: 11 }}
                      >
                        {t("Connect")}
                      </button>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}

          {/* Footer */}
          <div style={{ borderTop: "1px solid var(--border)", paddingTop: 8, display: "flex", justifyContent: "flex-end" }}>
            <button
              type="button"
              className="btn-discord secondary"
              onClick={goToNetworks}
              style={{ fontSize: 11, padding: "4px 10px" }}
            >
              {t("Manage Networks")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
