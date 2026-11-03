// SPDX-License-Identifier: AGPL-3.0-only
import page from "./VaultPage.tsx?raw";
import api from "../api/vault.ts?raw";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { VaultChecks, VaultEntry } from "../api/vault";
import {
  CLIPBOARD_CLEAR_MS,
  ChecksView,
  EntryRow,
  HIDE_AFTER_MS,
  NoticeStep,
  RecoveryKeyStep,
  SetupForm,
  UnlockForm,
  VaultSwitch,
  clearClipboardNow,
  copyAndClear,
  entryLabel,
  filterEntries,
  importSummary,
} from "./VaultPage";

// The text takes its language from the window's document.
(globalThis as { document?: unknown }).document ??= { documentElement: { lang: "en" } };

function entry(over: Partial<VaultEntry> & { id: string }): VaultEntry {
  return { origin: "https://example.com", name: "", username: "", created_ms: 1, changed_ms: 1, fillable: true, ...over };
}

const noop = () => {};

function fakeClipboard(initial = "") {
  const board = { text: initial };
  return {
    board,
    writeText: vi.fn(async (s: string) => {
      board.text = s;
    }),
    readText: vi.fn(async () => board.text),
  };
}

describe("the password vault page's text helpers", () => {
  it("labels an entry by its name, else its host", () => {
    expect(entryLabel({ name: "Work mail", origin: "https://mail.example.com/login" })).toBe("Work mail");
    expect(entryLabel({ name: "", origin: "https://mail.example.com/login" })).toBe("mail.example.com");
    expect(entryLabel({ name: "", origin: "android://abc" })).toBe("abc");
  });

  it("searches name, address and username, and lists by name", () => {
    const list = [
      entry({ id: "1", name: "Zed", username: "ann" }),
      entry({ id: "2", name: "Bank", origin: "https://bank.example", username: "bob" }),
      entry({ id: "3", name: "Mail", origin: "https://mail.example", username: "ANN" }),
    ];
    expect(filterEntries(list, "").map((e) => e.id)).toEqual(["2", "3", "1"]);
    expect(filterEntries(list, "ann").map((e) => e.id)).toEqual(["3", "1"]);
    expect(filterEntries(list, " bank.EXAMPLE ").map((e) => e.id)).toEqual(["2"]);
    expect(filterEntries(list, "nothing")).toEqual([]);
  });

  it("sums up an import", () => {
    expect(importSummary({ added: 3, updated: 0, unchanged: 0, skipped: 0 })).toBe("3 added");
    expect(importSummary({ added: 3, updated: 1, unchanged: 2, skipped: 4 })).toBe("3 added, 1 updated, 2 already saved, 4 skipped");
  });

  it("hides a shown password and empties the clipboard after 30 seconds", () => {
    expect(HIDE_AFTER_MS).toBe(30_000);
    expect(CLIPBOARD_CLEAR_MS).toBe(30_000);
  });
});

describe("copying a secret", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(async () => {
    await clearClipboardNow();
    vi.useRealTimers();
  });

  it("empties the clipboard after the wait", async () => {
    const clip = fakeClipboard();
    await copyAndClear("s3cret", clip, 30_000);
    expect(clip.board.text).toBe("s3cret");
    await vi.advanceTimersByTimeAsync(29_999);
    expect(clip.board.text).toBe("s3cret");
    await vi.advanceTimersByTimeAsync(2);
    expect(clip.board.text).toBe("");
  });

  it("leaves the clipboard alone when something else was copied meanwhile", async () => {
    const clip = fakeClipboard();
    await copyAndClear("s3cret", clip, 30_000);
    clip.board.text = "a shopping list";
    await vi.advanceTimersByTimeAsync(31_000);
    expect(clip.board.text).toBe("a shopping list");
  });

  it("empties it on demand, as when the vault locks", async () => {
    const clip = fakeClipboard();
    await copyAndClear("s3cret", clip, 30_000);
    await clearClipboardNow();
    expect(clip.board.text).toBe("");
    // Nothing left to clear later.
    clip.board.text = "later";
    await vi.advanceTimersByTimeAsync(31_000);
    expect(clip.board.text).toBe("later");
  });

  it("empties the first copy before starting the next", async () => {
    const clip = fakeClipboard();
    await copyAndClear("first", clip, 30_000);
    await copyAndClear("second", clip, 30_000);
    expect(clip.writeText.mock.calls.map((c) => c[0])).toEqual(["first", "", "second"]);
    await vi.advanceTimersByTimeAsync(31_000);
    expect(clip.board.text).toBe("");
  });

  it("empties the clipboard when reading it is refused", async () => {
    const clip = fakeClipboard();
    clip.readText.mockRejectedValue(new Error("denied"));
    await copyAndClear("s3cret", clip, 30_000);
    await vi.advanceTimersByTimeAsync(31_000);
    expect(clip.board.text).toBe("");
  });
});

describe("the password vault page", () => {
  it("shows the switch off or on", () => {
    const off = renderToStaticMarkup(<VaultSwitch enabled={false} busy={false} onTurnOn={noop} onTurnOff={noop} />);
    expect(off).toContain("Password vault");
    expect(off).not.toContain("checked");
    const on = renderToStaticMarkup(<VaultSwitch enabled busy={false} onTurnOn={noop} onTurnOff={noop} />);
    expect(on).toContain("checked");
  });

  it("holds back Turn on until the notice is accepted", () => {
    const html = renderToStaticMarkup(<NoticeStep busy={false} onAccept={noop} onCancel={noop} />);
    expect(html).toContain("responsible");
    expect(html).toContain("recovery key");
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Turn on</);
    expect(html).toContain("I have read this and accept it.");
  });

  it("asks twice for the master password, in password fields", () => {
    const html = renderToStaticMarkup(<SetupForm busy={false} onSetup={async () => {}} />);
    expect(html.match(/type="password"/g)).toHaveLength(2);
    expect(html).toContain("Master password");
    expect(html).toContain("Enter it again");
  });

  it("shows the recovery key once, with the I saved it step", () => {
    const html = renderToStaticMarkup(<RecoveryKeyStep recoveryKey="ABCDE-FGHJK-MNPQR-STVWX-YZ012-34567" onSaved={noop} />);
    expect(html).toContain("ABCDE-FGHJK-MNPQR-STVWX-YZ012-34567");
    expect(html).toContain("I saved it");
    expect(html).toContain("shown once");
  });

  it("offers a recovery route on the unlock screen", () => {
    const html = renderToStaticMarkup(<UnlockForm busy={false} onUnlock={async () => {}} onRecover={async () => {}} />);
    expect(html).toContain("The vault is locked.");
    expect(html).toContain("Forgot the master password?");
    expect(html.match(/type="password"/g)).toHaveLength(1);
    expect(html).toContain('value=""');
  });

  const row = (revealed: { password: string; notes: string } | null) =>
    renderToStaticMarkup(
      <ul>
        <EntryRow entry={entry({ id: "1", name: "Mail", username: "ann", fillable: false })} revealed={revealed} onShow={noop} onHide={noop} onCopyUser={noop} onCopyPassword={noop} onEdit={noop} onDelete={noop} />
      </ul>,
    );

  it("lists an entry without its password", () => {
    const html = row(null);
    expect(html).toContain("Mail");
    expect(html).toContain("ann");
    expect(html).toContain("Not filled");
    expect(html).toContain("Show");
    expect(html).toContain("Copy password");
    expect(html).not.toContain("Hidden again");
  });

  it("shows a revealed password with its notes and the hide time", () => {
    const html = row({ password: "river-stone-904", notes: "security answer: blue" });
    expect(html).toContain("river-stone-904");
    expect(html).toContain("security answer: blue");
    expect(html).toContain("Hidden again after 30 seconds.");
    expect(html).toContain("Hide");
  });

  it("lists weak, reused and old passwords by name, never by password", () => {
    const entries = [entry({ id: "a", name: "Mail", username: "ann" }), entry({ id: "b", name: "Bank" }), entry({ id: "c", name: "Forum" })];
    const report: VaultChecks = { weak: [["a", "very_weak"]], reused: [["b", "c"]], old: ["c"] };
    const html = renderToStaticMarkup(<ChecksView report={report} entries={entries} />);
    expect(html).toContain("Mail (ann)");
    expect(html).toContain("Very weak");
    expect(html).toContain("Bank, Forum");
    expect(html).toContain("Not changed in over a year");
  });

  it("says so when the checks find nothing", () => {
    const html = renderToStaticMarkup(<ChecksView report={{ weak: [], reused: [], old: [] }} entries={[]} />);
    expect(html).toContain("No weak, reused or old passwords found.");
  });

  it("keeps secrets out of browser storage and the console", () => {
    for (const [file, source] of [["VaultPage.tsx", page], ["api/vault.ts", api]]) {
      expect(source, file).not.toMatch(/localStorage|sessionStorage|indexedDB|console\./);
    }
  });
});
