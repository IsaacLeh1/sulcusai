// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useState } from "react";
import { api, errorText, type LinkedMachine } from "../api";
import { bytes } from "../format";
import { t } from "../i18n";
import type { PushToast } from "./Toasts";

const GIB = 1024 ** 3;

/** Other machines lending their graphics memory (two DGX Sparks linked, say):
 * each runs llama.cpp's `rpc-server`, and models are fitted to and split
 * across all the memory together. Nothing is linked until added here. */
export function LinkedMachines({ toast, onChanged }: { toast: PushToast; onChanged: () => void }) {
  const [list, setList] = useState<LinkedMachine[]>([]);
  const [adding, setAdding] = useState(false);
  const [addr, setAddr] = useState("");
  const [gb, setGb] = useState("");

  useEffect(() => {
    api.linkedMachines().then(setList).catch(() => {});
  }, []);

  const save = async (next: LinkedMachine[]) => {
    try {
      setList(await api.setLinkedMachines(next));
      onChanged();
      return true;
    } catch (e) {
      toast(errorText(e), "error");
      return false;
    }
  };

  const add = async () => {
    const size = Number(gb);
    if (!(size > 0)) return toast(t("Say how much graphics memory it lends, in GB."), "error");
    if (await save([...list, { addr: addr.trim(), vram_bytes: Math.round(size * GIB), bw_gbs: 0 }])) {
      setAdding(false);
      setAddr("");
      setGb("");
    }
  };

  return (
    <div className="gpu-reasons">
      <strong className="small">{t("Linked machines")}</strong>
      {list.length === 0 && !adding && <p className="small muted">{t("None. Link another machine running llama.cpp's rpc-server to use its memory too.")}</p>}
      <ul>
        {list.map((m) => (
          <li key={m.addr} className="small">
            {t("{addr}: {size}", { addr: m.addr, size: bytes(m.vram_bytes) })}{" "}
            <button className="link" onClick={() => save(list.filter((x) => x.addr !== m.addr))}>
              {t("Remove")}
            </button>
          </li>
        ))}
      </ul>
      {adding ? (
        <div className="row small">
          <input aria-label={t("Address and port")} placeholder="192.168.100.2:50052" value={addr} onChange={(e) => setAddr(e.target.value)} />
          <input aria-label={t("Memory it lends, in GB")} placeholder={t("GB")} inputMode="decimal" value={gb} onChange={(e) => setGb(e.target.value)} size={5} />
          <button className="btn small" onClick={add}>
            {t("Link")}
          </button>
          <button className="btn ghost small" onClick={() => setAdding(false)}>
            {t("Cancel")}
          </button>
        </div>
      ) : (
        <button className="link small" onClick={() => setAdding(true)}>
          {t("Link a machine")}
        </button>
      )}
    </div>
  );
}
