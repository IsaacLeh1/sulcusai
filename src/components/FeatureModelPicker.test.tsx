// SPDX-License-Identifier: AGPL-3.0-only
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { FeatureModelPanel, FeatureModelPicker, verdictText, type FitPickerView } from "./FeatureModelPicker";

// The text takes its language from the window's document.
(globalThis as { document?: unknown }).document ??= { documentElement: { lang: "en" } };

const small = { model_id: "tiny", name: "Tiny 1.7B", params_b: 1.7, refused: ["Too small for Mail: it has 1.7B parameters and the job needs at least 4B."], tested: null };
const big = { model_id: "big", name: "Big 8B", params_b: 8, refused: [], tested: { passed: 3, total: 3, ok: true } };
const advice = { model_id: "qwen3-4b", name: "Qwen3 4B", params_b: 4, quant: "Q4_K_M", size: 2.5 * 1024 ** 3, est_tps: 80 };

function view(over: Partial<FitPickerView> = {}): FitPickerView {
  return { feature: "mail", minimum_params_b: 4, minimum_ctx: 16384, current: null, installed: [small, big], advice: [], ...over };
}

function panel(v: FitPickerView, extra: Partial<Parameters<typeof FeatureModelPanel>[0]> = {}) {
  return renderToStaticMarkup(
    <FeatureModelPanel view={v} testing={null} report={null} reportFor={null} downloading={[]} onChoose={vi.fn()} onTest={vi.fn()} onDownload={vi.fn()} {...extra} />,
  );
}

describe("FeatureModelPanel", () => {
  it("greys out a refused model and says why, and offers a test only for the others", () => {
    const html = panel(view());
    expect(html).toContain("Too small for Mail");
    expect(html).toMatch(/<button[^>]*disabled=""[^>]*>Tiny 1.7B/);
    expect(html).not.toMatch(/<button[^>]*disabled=""[^>]*>Big 8B/);
    expect(html.match(/Test it/g)).toHaveLength(1);
    expect(html).toContain("Passed the test (3 of 3)");
  });

  it("marks the chosen model", () => {
    const html = panel(view({ current: "big" }));
    expect(html).toMatch(/aria-checked="true"[^>]*>Big 8B/);
    expect(html).toMatch(/aria-checked="false"[^>]*>The default model/);
  });

  it("shows advice with a download only when no installed model is allowed", () => {
    expect(panel(view({ advice: [advice] }))).not.toContain("Download");
    const html = panel(view({ installed: [small], advice: [advice] }));
    expect(html).toContain("Qwen3 4B");
    expect(html).toContain("2.5 GB download");
    expect(html).toContain("Download");
  });

  it("says so when nothing in the catalog fits", () => {
    expect(panel(view({ installed: [], advice: [] }))).toContain("No model in the catalog");
  });

  it("shows a failed test and what the answers lacked", () => {
    const failed = { ...big, tested: { passed: 1, total: 3, ok: false } };
    const html = panel(view({ installed: [failed] }), {
      reportFor: "big",
      report: { verdict: failed.tested, results: [{ id: "a", passed: true, problem: null }, { id: "b", passed: false, problem: "Its answer doesn't say \"friday\"." }] },
    });
    expect(html).toContain("Failed the test (1 of 3)");
    expect(html).toContain("Answered well");
    expect(html).toContain("doesn&#x27;t say");
  });

  it("disables every test button while one runs", () => {
    const html = panel(view(), { testing: "big" });
    expect(html).toContain("Testing…");
  });

  it("words the verdict", () => {
    expect(verdictText({ passed: 2, total: 3, ok: true })).toBe("Passed the test (2 of 3)");
  });
});

describe("FeatureModelPicker", () => {
  it("is a gear button until it is opened", () => {
    const html = renderToStaticMarkup(<FeatureModelPicker feature="calendar" />);
    expect(html).toContain("Choose the model for Calendar");
    expect(html).not.toContain("role=\"dialog\"");
  });
});
