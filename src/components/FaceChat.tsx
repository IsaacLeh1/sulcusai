// SPDX-License-Identifier: AGPL-3.0-only
// Voice chat with a face: each spoken reply is also made into the chosen picture saying it (Ditto), shown muted beside the voice.
import { useEffect, useRef, useState } from "react";
import { api, errorText, on, type MediaItem } from "../api";
import { create, loadFaceChat, saveFaceChat, type FaceChat as FaceChatSettings } from "../api/create";
import { t } from "../i18n";
import { useMediaUrl } from "../media";
import type { PushToast } from "./Toasts";

/** Shown in the voice panel while a voice chat runs, when it's turned on. */
export function FaceInVoice({ chatId, toast }: { chatId: string; toast: PushToast }) {
  const [settings] = useState(loadFaceChat);
  const [ready, setReady] = useState(false);
  const [latest, setLatest] = useState<MediaItem | null>(null);
  const [making, setMaking] = useState(false);
  const jobs = useRef(new Set<string>());

  useEffect(() => {
    if (!settings.on) return;
    create.faceChatCheck(settings).then(
      () => setReady(true),
      (e) => toast(errorText(e), "error"),
    );
  }, [settings, toast]);

  useEffect(() => {
    if (!ready || !settings.picture) return;
    const picture = settings.picture;
    const said = on("voice", (e) => {
      if (e.chat_id !== chatId || e.state !== "replied") return;
      setMaking(true);
      api
        .mediaStart({ op: "talk", prompt: "", source: picture, say: e.text, chat_id: chatId })
        .then((id) => jobs.current.add(id))
        .catch((err) => {
          setMaking(false);
          toast(errorText(err), "error");
        });
    });
    const done = on("media:done", (d) => {
      if (!jobs.current.delete(d.job_id)) return;
      setMaking(false);
      const v = d.items?.find((i) => i.kind === "video");
      if (v) setLatest(v);
    });
    return () => {
      said.then((un) => un());
      done.then((un) => un());
    };
  }, [ready, settings.picture, chatId, toast]);

  if (!settings.on || !ready) return null;
  return (
    <div className="face-chat">
      {latest ? <FaceVideo item={latest} /> : <span className="muted small">{t("The face appears after the first reply.")}</span>}
      {making && <span className="muted small">{t("Making the face say it...")}</span>}
    </div>
  );
}

function FaceVideo({ item }: { item: MediaItem }) {
  const src = useMediaUrl(item);
  if (!src) return null;
  // The voice is already playing; the face is shown without its own sound.
  return <video key={item.id} src={src} autoPlay muted playsInline style={{ maxWidth: 220, borderRadius: 8 }} />;
}

/** Settings for it, in Studio > Create. */
export function FaceChatSettings({ pictures, toast }: { pictures: MediaItem[]; toast: PushToast }) {
  const [s, setS] = useState<FaceChatSettings>(loadFaceChat);
  const update = async (next: FaceChatSettings) => {
    if (next.on) {
      try {
        await create.faceChatCheck(next);
      } catch (e) {
        toast(errorText(e), "error");
        next = { ...next, on: false };
      }
    }
    setS(next);
    saveFaceChat(next);
  };
  return (
    <div className="stack">
      <p className="muted">{t("In voice chat, each reply is also made into this picture saying it. It shows a moment after the voice, since making the face takes time.")}</p>
      <label className="field">
        <span>{t("Picture of the face")}</span>
        <select value={s.picture ?? ""} onChange={(e) => update({ ...s, picture: e.target.value || null, on: false })}>
          <option value="">{t("Choose...")}</option>
          {pictures.slice(0, 200).map((p) => (
            <option key={p.id} value={p.id}>
              {(p.title || p.prompt || p.id).slice(0, 70)}
            </option>
          ))}
        </select>
      </label>
      <label className="field">
        <span>{t("Whose face is it?")}</span>
        <select value={s.claim.face ?? ""} onChange={(e) => update({ ...s, on: false, claim: { ...s.claim, face: (e.target.value || undefined) as FaceChatSettings["claim"]["face"] } })}>
          <option value="">{t("Choose...")}</option>
          <option value="nobody">{t("No one real (made up or drawn)")}</option>
          <option value="myself">{t("Mine")}</option>
          <option value="someone">{t("Someone else, real")}</option>
        </select>
      </label>
      {s.claim.face === "someone" && (
        <label className="check">
          <input type="checkbox" checked={!!s.claim.permission_from_them} onChange={(e) => update({ ...s, on: false, claim: { ...s.claim, permission_from_them: e.target.checked } })} />
          {t("I have their permission to make their face talk")}
        </label>
      )}
      <label className="check">
        <input type="checkbox" checked={s.on} disabled={!s.picture || !s.claim.face} onChange={(e) => update({ ...s, on: e.target.checked })} />
        {t("Show this face in voice chat")}
      </label>
    </div>
  );
}
