// SPDX-License-Identifier: AGPL-3.0-only
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => Promise.resolve(null)) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

import { YourDataSettings } from "./YourDataSettings";
import { settingsSections } from "./SettingsNav";

describe("Your data", () => {
  it("offers export, delete and the places to report crimes", () => {
    const html = renderToStaticMarkup(<YourDataSettings toast={() => {}} />);
    expect(html).toContain("Export everything");
    expect(html).toContain("Delete everything");
    expect(html).toContain("Report a crime");
    expect(html).toContain("report.cybertip.org");
  });

  it("has its own place in the Settings sidebar", () => {
    expect(settingsSections(new Set()).map((s) => s.id)).toContain("your-data");
  });
});
