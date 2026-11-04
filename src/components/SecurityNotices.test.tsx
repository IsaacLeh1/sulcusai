// SPDX-License-Identifier: AGPL-3.0-only
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { SecurityNotice } from "../api/security";
import { SecurityNoticesView } from "./SecurityNotices";

(globalThis as { document?: unknown }).document ??= { documentElement: { lang: "en" } };

function notice(over: Partial<SecurityNotice>): SecurityNotice {
  return { status: "current", last_check: 0, last_result: "new", last_error: null, sequence: 3, issued: 0, update_needed: false, lowest_safe_version: "0.9.50", revoked: [], messages: [], ...over };
}

const show = (n: SecurityNotice) => renderToStaticMarkup(<SecurityNoticesView notice={n} busy={false} onCheck={() => {}} />);

describe("security notices on the Security page", () => {
  it("says there is no signing key yet and offers no check", () => {
    const html = show(notice({ status: "no_key", last_result: null }));
    expect(html).toContain("No signing key yet");
    expect(html).not.toContain("<button");
  });

  it("says nothing has arrived when a key exists but no notice was accepted", () => {
    expect(show(notice({ status: "waiting", last_result: "none" }))).toContain("No security notice has been published yet.");
  });

  it("shows a rejected file's reason and keeps showing nothing from it", () => {
    const html = show(notice({ status: "waiting", last_result: "rejected", last_error: "The security notice's signature doesn't match." }));
    expect(html).toContain("signature doesn&#x27;t match");
    expect(html).not.toContain("Nothing to report");
  });

  it("shows the update warning, withdrawn programs and messages, worst first as the backend sorted them", () => {
    const html = show(
      notice({
        update_needed: true,
        revoked: [{ kind: "engine", id: "llama-b1", reason: "unsafe loading" }],
        messages: [{ severity: "critical", text: "Update now." }, { severity: "info", text: "All good." }],
      }),
    );
    expect(html).toContain("below the lowest safe version, 0.9.50");
    expect(html).toContain("llama-b1 has been withdrawn");
    expect(html).toContain("unsafe loading");
    expect(html.indexOf("Update now.")).toBeLessThan(html.indexOf("All good."));
  });

  it("reports an empty current notice", () => {
    expect(show(notice({}))).toContain("Nothing to report. The latest notice is number 3.");
  });
});
