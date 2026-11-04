// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { errorText } from "../api";
import { t } from "../i18n";
import { fraction, remoteConnect, remoteDisconnect, remoteSend, type RemoteInput } from "../api/sharing";

/** Controlling a friend's PC: read them the code, then their screen shows here and takes this mouse and keyboard. */
export function RemoteView({ peer, onClose }: { peer: string; onClose: () => void }) {
  const [code, setCode] = useState("");
  const [frame, setFrame] = useState("");
  const [err, setErr] = useState("");
  const img = useRef<HTMLImageElement>(null);

  useEffect(() => {
    const un = [
      listen<{ code: string }>("remote:code", (e) => setCode(e.payload.code)),
      listen<string>("remote:frame", (e) => setFrame(e.payload)),
      listen("remote:ended", () => onClose()),
    ];
    remoteConnect(peer).catch((e) => setErr(errorText(e)));
    return () => {
      un.forEach((p) => p.then((f) => f()));
      remoteDisconnect().catch(() => {});
    };
  }, [peer, onClose]);

  const send = (i: RemoteInput) => remoteSend(i).catch(() => {});
  const at = (e: React.MouseEvent) => (img.current ? fraction(e.clientX, e.clientY, img.current.getBoundingClientRect()) : { x: 0, y: 0 });

  return (
    <div className="remote-view">
      <div className="row">
        <strong>{frame ? t("Controlling their PC") : t("Read them this code: {code}", { code: code || "..." })}</strong>
        <button onClick={onClose}>{t("Stop")}</button>
      </div>
      {err && <p className="small">{err}</p>}
      {frame && (
        <img
          ref={img}
          tabIndex={0}
          alt={t("Their screen")}
          src={`data:image/jpeg;base64,${frame}`}
          draggable={false}
          onMouseMove={(e) => send({ do: "move", ...at(e) })}
          onMouseDown={(e) => {
            e.preventDefault();
            send({ do: "move", ...at(e) });
            send({ do: "click", button: e.button, down: true });
          }}
          onMouseUp={(e) => send({ do: "click", button: e.button, down: false })}
          onContextMenu={(e) => e.preventDefault()}
          onWheel={(e) => send({ do: "scroll", dy: e.deltaY })}
          onKeyDown={(e) => {
            e.preventDefault();
            send({ do: "key", code: e.code, down: true });
          }}
          onKeyUp={(e) => {
            e.preventDefault();
            send({ do: "key", code: e.code, down: false });
          }}
        />
      )}
    </div>
  );
}
