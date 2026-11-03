// SPDX-License-Identifier: AGPL-3.0-only
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { AccessKeysBody, offeredScopes, SERVER_SCOPES, type AccessKey, type AccessKeysView, type ScopeInfo } from "./AccessKeys";

(globalThis as { document?: unknown }).document ??= { documentElement: { lang: "en" } };

const scope = (id: string, title: string): ScopeInfo => ({ scope: id, id, title, description: `${title} words`, risk: "low" });
const all = [scope("use_models", "Use models"), scope("use_cloud_models", "Use cloud models"), scope("use_assistant", "Use the assistant"), scope("read_mail", "Read mail")];
const key = (over: Partial<AccessKey> = {}): AccessKey => ({ id: "k1", name: "Editor", hint: "...ab12", scopes: [all[0]], created_at: 1, expires_at: null, last_used_at: null, status: "active", legacy: false, ...over });
const none = () => {};

function html(v: AccessKeysView, secret: string | null = null, picked: string[] = []) {
  return renderToStaticMarkup(
    <AccessKeysBody v={v} offered={offeredScopes(v.scopes)} secret={secret} name="" picked={picked} onName={none} onPick={none} onMake={none} onRevoke={none} onCopy={none} onDone={none} />,
  );
}

describe("access keys", () => {
  it("offers only what the server checks today", () => {
    expect(SERVER_SCOPES).toEqual(["use_models", "use_cloud_models", "use_assistant"]);
    expect(offeredScopes(all).map((s) => s.id)).toEqual(["use_models", "use_cloud_models", "use_assistant"]);
    const out = html({ keys: [], scopes: all, log: [] });
    expect(out).toContain("Use cloud models");
    expect(out).not.toContain("Read mail");
  });

  it("starts with nothing ticked", () => {
    const out = html({ keys: [], scopes: all, log: [] });
    expect(out).not.toContain("checked");
    expect(html({ keys: [], scopes: all, log: [] }, null, ["use_models"])).toContain("checked");
  });

  it("shows a new key's secret only while it's being copied", () => {
    expect(html({ keys: [key()], scopes: all, log: [] }, "sk-sulcus-akSECRET")).toContain("sk-sulcus-akSECRET");
    expect(html({ keys: [key()], scopes: all, log: [] })).not.toContain("sk-sulcus-ak");
  });

  it("offers turning off only for working keys, and lists refusals", () => {
    expect(html({ keys: [key()], scopes: all, log: [] })).toContain("Turn off");
    const off = html({ keys: [key({ status: "revoked" })], scopes: all, log: [{ at: 2, app: "", scope: "use_models", outcome: { kind: "denied", reason: "unknown_key" } }] });
    expect(off).toContain("Turned off");
    expect(off).not.toContain(">Turn off<");
    expect(off).toContain("Refused lately (1)");
  });
});
