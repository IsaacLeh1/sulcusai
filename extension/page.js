// SPDX-License-Identifier: AGPL-3.0-only
// Runs in the page (the extension's isolated world): reads it as numbered
// elements and acts on them. The same rules as SulcusAI's own browser.
(() => {
  if (globalThis.__sulcus) return;

  const shown = (el) => {
    const r = el.getBoundingClientRect();
    const s = getComputedStyle(el);
    return r.width > 0 && r.height > 0 && s.visibility !== "hidden" && s.display !== "none";
  };
  const inView = (el) => {
    const r = el.getBoundingClientRect();
    return r.bottom > 0 && r.top < innerHeight;
  };
  const label = (el) =>
    (el.getAttribute("aria-label") || el.innerText || el.value || el.placeholder || el.title || el.getAttribute("alt") || el.name || "").replace(/\s+/g, " ").trim().slice(0, 80);
  const byRef = (ref) => document.querySelector(`[data-sulcus-ref="${ref}"]`);
  const SEL =
    'a[href], button, input:not([type=hidden]), select, textarea, summary, [role=button], [role=link], [role=tab], [role=menuitem], [role=checkbox], [role=option], [role=searchbox], [role=textbox], [contenteditable=""], [contenteditable=true]';

  function banner() {
    if (document.getElementById("__sulcus_banner")) return;
    const b = document.createElement("div");
    b.id = "__sulcus_banner";
    b.textContent = "SulcusAI is using this tab. Stop it from the SulcusAI chat.";
    b.style.cssText =
      "position:fixed;left:50%;bottom:12px;transform:translateX(-50%);z-index:2147483647;background:#0f766e;color:#fff;font:13px system-ui,sans-serif;padding:6px 14px;border-radius:99px;box-shadow:0 2px 8px rgba(0,0,0,.3);pointer-events:none";
    (document.body || document.documentElement).appendChild(b);
  }

  function read() {
    banner();
    const MAX_EL = 150;
    document.querySelectorAll("[data-sulcus-ref]").forEach((e) => e.removeAttribute("data-sulcus-ref"));
    const all = [...document.querySelectorAll(SEL)].filter((el) => shown(el) && el.id !== "__sulcus_banner");
    const ordered = all.filter(inView).concat(all.filter((el) => !inView(el)));
    const elements = ordered.slice(0, MAX_EL).map((el, i) => {
      const ref = i + 1;
      el.setAttribute("data-sulcus-ref", String(ref));
      const tag = el.tagName.toLowerCase();
      const o = { ref, kind: tag === "input" ? "input:" + (el.type || "text") : el.getAttribute("role") || tag, label: label(el) };
      if (tag === "a") o.href = (el.getAttribute("href") || "").slice(0, 120);
      if (tag === "select") o.options = [...el.options].slice(0, 12).map((x) => x.text.trim());
      if ((tag === "input" || tag === "textarea") && el.type !== "password" && el.value) o.value = String(el.value).slice(0, 80);
      return o;
    });
    const b = document.getElementById("__sulcus_banner");
    const text = (document.body ? document.body.innerText : "").replace(b ? b.textContent : "\u0000", "").replace(/\n{3,}/g, "\n\n").trim();
    return { url: location.href, title: document.title, text, elements, more: all.length > MAX_EL };
  }

  function target(ref) {
    const el = byRef(ref);
    if (!el) return null;
    el.scrollIntoView({ block: "center", inline: "center" });
    const b = el.getBoundingClientRect();
    const tag = el.tagName.toLowerCase();
    const type = (el.getAttribute("type") || "").toLowerCase();
    const form = el.form || el.closest("form");
    return {
      x: b.left + b.width / 2,
      y: b.top + b.height / 2,
      tag,
      type,
      label: label(el),
      href: tag === "a" ? el.href : null,
      form: form ? { method: (form.getAttribute("method") || "get").toLowerCase(), action: form.action || location.href, search: form.getAttribute("role") === "search" || !!form.querySelector("input[type=search]") } : null,
      autocomplete: (el.getAttribute("autocomplete") || "").toLowerCase(),
      name: ((el.name || "") + " " + (el.id || "")).toLowerCase(),
      host: location.host,
    };
  }

  function click(ref) {
    const el = byRef(ref);
    if (!el) return false;
    el.scrollIntoView({ block: "center" });
    el.focus?.();
    el.click();
    return true;
  }

  /** Sets a field the way typing would, so pages built with frameworks notice. */
  function type(ref, text, submit) {
    const el = byRef(ref);
    if (!el) return false;
    el.focus();
    if (el.tagName === "SELECT") {
      const o = [...el.options].find((x) => x.text.trim().toLowerCase() === text.toLowerCase() || x.value === text);
      if (!o) return "no-choice";
      el.value = o.value;
    } else if (el.isContentEditable) {
      el.textContent = text;
    } else {
      const proto = el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, "value").set.call(el, text);
    }
    el.dispatchEvent(new Event("input", { bubbles: true }));
    el.dispatchEvent(new Event("change", { bubbles: true }));
    if (submit) {
      const opts = { key: "Enter", code: "Enter", keyCode: 13, which: 13, bubbles: true, cancelable: true };
      const go = el.dispatchEvent(new KeyboardEvent("keydown", opts));
      el.dispatchEvent(new KeyboardEvent("keyup", opts));
      const form = el.form || el.closest("form");
      if (go && form) form.requestSubmit ? form.requestSubmit() : form.submit();
    }
    return true;
  }

  globalThis.__sulcus = { read, target, click, type, banner };
})();
