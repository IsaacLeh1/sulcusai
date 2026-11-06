// SPDX-License-Identifier: AGPL-3.0-only
// Profiles: the strip at the bottom left, the editor, and the card shown for
// someone's name. Each person's profile lives on their own PC and reaches
// others peer to peer.
import { useEffect, useRef, useState } from "react";
import { errorText } from "../../api";
import { t } from "../../i18n";
import type { PushToast } from "../../components/Toasts";
import { SafeLink } from "./ChatParts";
import { EMPTY_PROFILE, social, type Presence, type Profile } from "./socialApi";

export const PRESENCE: [Presence, string][] = [
  ["online", "Online"],
  ["idle", "Idle"],
  ["do_not_disturb", "Do not disturb"],
  ["invisible", "Invisible (shown as offline)"],
];

/** The name someone goes by in a group: their name there, their profile name, or the group's. */
export function nameIn(p: Profile | undefined, group: string, fallback: string): string {
  return p?.nicknames?.[group] || p?.name || fallback;
}

export const FRAMES: [string, string][] = [
  ["", "None"],
  ["glow", "Glow"],
  ["gold", "Gold"],
  ["rainbow", "Rainbow"],
  ["dashed", "Dashed"],
];

export function Avatar({ name, p, size = 36 }: { name: string; p?: Profile; size?: number }) {
  return (
    <span className={p?.frame ? `social-avatar frame-${p.frame}` : "social-avatar"} style={{ width: size, height: size }} aria-hidden>
      {p?.picture ? <img src={p.picture} alt="" /> : (p?.name || name).slice(0, 1).toUpperCase()}
      {p && <span className={`social-presence ${p.presence}`} />}
    </span>
  );
}

/** Shrinks a picture to fit `max` pixels on its longest side, as a small JPEG or PNG. */
function shrink(file: File, max: number, animated = false): Promise<string> {
  // A small GIF is kept as it is, so it still moves.
  if (animated && file.type === "image/gif" && file.size <= 180_000)
    return new Promise((ok, fail) => {
      const r = new FileReader();
      r.onload = () => ok(String(r.result));
      r.onerror = () => fail(r.error);
      r.readAsDataURL(file);
    });
  return new Promise((ok, fail) => {
    const img = new Image();
    img.onload = () => {
      const k = Math.min(1, max / Math.max(img.width, img.height));
      const c = document.createElement("canvas");
      c.width = Math.round(img.width * k);
      c.height = Math.round(img.height * k);
      c.getContext("2d")?.drawImage(img, 0, 0, c.width, c.height);
      URL.revokeObjectURL(img.src);
      ok(c.toDataURL(file.type === "image/png" ? "image/png" : "image/jpeg", 0.85));
    };
    img.onerror = () => fail(new Error(t("That picture couldn't be read.")));
    img.src = URL.createObjectURL(file);
  });
}

export function ProfileEditor({ start, myName, onClose, toast, extras = false, group }: { start: Profile; myName: string; onClose: (saved?: Profile) => void; toast: PushToast; extras?: boolean; group?: { id: string; name: string } }) {
  const [p, setP] = useState<Profile>({ ...start, name: start.name || myName });
  const pic = useRef<HTMLInputElement>(null);
  const banner = useRef<HTMLInputElement>(null);
  const set = (k: keyof Profile) => (e: { target: { value: string } }) => setP((x) => ({ ...x, [k]: e.target.value }));
  const save = async () => {
    try {
      const sent = await social.setProfile(p);
      toast(sent > 0 ? t("Saved, and sent to {n} people you share a group with.", { n: sent }) : t("Saved. It's sent to people in your groups when they're connected."));
      onClose(p);
    } catch (e) {
      toast(errorText(e));
    }
  };
  return (
    <div className="social-modal" role="dialog" aria-label={t("Your profile")} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="social-modal-box">
        <ProfileCardView p={p} name={myName} />
        <p className="small muted">{t("Your profile stays on this PC and goes only to the people you share a group with, straight from your PC. Nothing is stored on a server.")}</p>
        <label>
          {t("Name")}
          <input value={p.name} onChange={set("name")} maxLength={40} />
        </label>
        <label>
          {t("Pronouns")}
          <input value={p.pronouns} onChange={set("pronouns")} maxLength={40} />
        </label>
        <label>
          {t("Status")}
          <input value={p.status} onChange={set("status")} maxLength={100} placeholder={t("What you're up to")} />
        </label>
        <label>
          {t("Shown as")}
          <select value={p.presence} onChange={(e) => setP((x) => ({ ...x, presence: e.target.value as Presence }))}>
            {PRESENCE.map(([k, l]) => (
              <option key={k} value={k}>
                {t(l)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("About")}
          <textarea value={p.about} onChange={set("about")} maxLength={500} rows={4} />
        </label>
        <label>
          {t("Links (one a line, up to five)")}
          <textarea value={p.links.join("\n")} onChange={(e) => setP((x) => ({ ...x, links: e.target.value.split("\n") }))} rows={3} />
        </label>
        {extras && (
          <fieldset className="profile-extras">
            <legend>{t("Extras")}</legend>
            {group && (
              <label>
                {t("Your name in {group}", { group: group.name })}
                <input
                  value={p.nicknames?.[group.id] ?? ""}
                  maxLength={32}
                  placeholder={p.name || myName}
                  onChange={(e) => setP((x) => ({ ...x, nicknames: { ...x.nicknames, [group.id]: e.target.value } }))}
                />
              </label>
            )}
            <div className="row">
              <label className="row">
                {t("Profile colours")}
                <input type="color" value={p.accent || "#5865f2"} onChange={set("accent")} aria-label={t("First colour")} />
                <input type="color" value={p.accent2 || p.accent || "#5865f2"} onChange={set("accent2")} aria-label={t("Second colour")} />
              </label>
              {(p.accent || p.accent2) && (
                <button className="link small" onClick={() => setP((x) => ({ ...x, accent: "", accent2: "" }))}>
                  {t("Plain")}
                </button>
              )}
            </div>
            <label>
              {t("Picture frame")}
              <select value={p.frame ?? ""} onChange={set("frame")}>
                {FRAMES.map(([k, l]) => (
                  <option key={k} value={k}>
                    {t(l)}
                  </option>
                ))}
              </select>
            </label>
            <p className="small muted">{t("A GIF picture under 180 KB stays animated.")}</p>
          </fieldset>
        )}
        <div className="row">
          <button onClick={() => pic.current?.click()}>{t("Picture")}</button>
          <button onClick={() => banner.current?.click()}>{t("Banner")}</button>
          {(p.picture || p.banner) && <button onClick={() => setP((x) => ({ ...x, picture: "", banner: "" }))}>{t("Remove pictures")}</button>}
          <input ref={pic} type="file" accept="image/*" hidden onChange={async (e) => e.target.files?.[0] && setP({ ...p, picture: await shrink(e.target.files[0], 256, extras) })} />
          <input ref={banner} type="file" accept="image/*" hidden onChange={async (e) => e.target.files?.[0] && setP({ ...p, banner: await shrink(e.target.files[0], 640) })} />
        </div>
        <div className="row">
          <button className="primary" onClick={save}>
            {t("Save")}
          </button>
          <button onClick={() => onClose()}>{t("Cancel")}</button>
        </div>
      </div>
    </div>
  );
}

function ProfileCardView({ p, name }: { p?: Profile; name: string }) {
  const pr = p ?? EMPTY_PROFILE;
  return (
    <div className={pr.accent ? "social-card themed" : "social-card"} style={pr.accent ? { background: `linear-gradient(160deg, ${pr.accent}, ${pr.accent2 || pr.accent})` } : undefined}>
      <div className="social-banner" style={pr.banner ? { backgroundImage: `url(${pr.banner})` } : pr.accent ? { background: pr.accent } : undefined} />
      <Avatar name={name} p={p} size={64} />
      <strong>{pr.name || name}</strong>
      {pr.pronouns && <span className="small muted"> {pr.pronouns}</span>}
      {pr.status && <div className="small">{pr.status}</div>}
      {pr.about && <p className="small chat-text">{pr.about}</p>}
      {pr.links.filter(Boolean).map((l) => (
        <div key={l} className="small">
          <SafeLink href={l}>{l}</SafeLink>
        </div>
      ))}
    </div>
  );
}

/** Someone's profile over the window, from their name or picture. */
export function ProfileCard({ p, name, onClose, onMessage }: { p?: Profile; name: string; onClose: () => void; onMessage?: () => void }) {
  return (
    <div className="social-modal" role="dialog" aria-label={name} onClick={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="social-modal-box" onClick={(e) => e.stopPropagation()}>
        <ProfileCardView p={p} name={name} />
        {!p && <p className="small muted">{t("They haven't shared a profile yet.")}</p>}
        <div className="row">
          {onMessage && <button onClick={onMessage}>{t("Message")}</button>}
          <button onClick={onClose}>{t("Close")}</button>
        </div>
      </div>
    </div>
  );
}

/** Bottom left: your picture, name and status, mute, deafen and your profile. */
export function ProfileStrip({
  myName,
  muted,
  deafened,
  setMuted,
  setDeafened,
  toast,
  extras = false,
  group,
}: {
  myName: string;
  muted: boolean;
  deafened: boolean;
  setMuted: (m: boolean) => void;
  setDeafened: (d: boolean) => void;
  toast: PushToast;
  extras?: boolean;
  group?: { id: string; name: string };
}) {
  const [p, setP] = useState<Profile | null>(null);
  const [editing, setEditing] = useState(false);
  useEffect(() => {
    social
      .profile()
      .then((x) => {
        setP(x);
        // Sent again once a session, so people who joined since get it.
        if (x.at > 0) social.setProfile(x).catch(() => {});
      })
      .catch(() => setP(EMPTY_PROFILE));
  }, []);
  const shown = p ?? EMPTY_PROFILE;
  return (
    <div className="social-profile">
      <button className="social-profile-me" onClick={() => setEditing(true)} title={t("Your profile")}>
        <Avatar name={myName} p={shown} size={32} />
        <span className="social-profile-text">
          <strong>{group ? nameIn(shown, group.id, myName) : shown.name || myName}</strong>
          <span className="small muted">{shown.status || t(PRESENCE.find(([k]) => k === shown.presence)?.[1] ?? "Online")}</span>
        </span>
      </button>
      <button className={muted ? "danger" : ""} aria-pressed={muted} onClick={() => setMuted(!muted)} title={t("Mute your microphone in calls")}>
        {muted ? t("Unmute") : t("Mute")}
      </button>
      <button className={deafened ? "danger" : ""} aria-pressed={deafened} onClick={() => setDeafened(!deafened)} title={t("Hear nobody in calls")}>
        {deafened ? t("Undeafen") : t("Deafen")}
      </button>
      <button onClick={() => setEditing(true)} title={t("Profile settings")}>
        {t("Profile")}
      </button>
      {editing && (
        <ProfileEditor
          start={shown}
          myName={myName}
          toast={toast}
          extras={extras}
          group={group}
          onClose={(saved) => {
            setEditing(false);
            if (saved) setP(saved);
          }}
        />
      )}
    </div>
  );
}
