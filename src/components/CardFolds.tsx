// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, type RefObject } from "react";

/** Settings cards folded up (by their heading), remembered on this PC. */
const KEY = "sulcus.settings.folded";

function load(): Set<string> {
  try {
    return new Set(JSON.parse(localStorage.getItem(KEY) ?? "[]") as string[]);
  } catch {
    return new Set();
  }
}

function keep(folded: Set<string>) {
  try {
    localStorage.setItem(KEY, JSON.stringify([...folded]));
  } catch {
    /* not kept */
  }
}

function setFolded(card: HTMLElement, fold: boolean, folded: Set<string>) {
  card.classList.toggle("collapsed", fold);
  const name = card.dataset.fold ?? "";
  if (fold) folded.add(name);
  else folded.delete(name);
  card.querySelector<HTMLElement>(".card-fold-head")?.setAttribute("aria-expanded", String(!fold));
}

/** Each card on the page folds up by its heading (a click, or Enter); the
 * cards a page adds later (as they load) too. */
export function useCardFolds(pageRef: RefObject<HTMLElement | null>) {
  useEffect(() => {
    const root = pageRef.current;
    if (!root) return;
    const setup = () => {
      const folded = load();
      root.querySelectorAll<HTMLElement>("section.card").forEach((card) => {
        if (card.dataset.fold !== undefined) return;
        const h = card.querySelector<HTMLElement>("h2");
        if (!h) return;
        // What stays in sight when folded: the card's part holding the heading.
        let top: HTMLElement = h;
        while (top.parentElement && top.parentElement !== card) top = top.parentElement;
        if (top.parentElement !== card) return;
        card.dataset.fold = h.textContent?.trim() ?? "";
        top.classList.add("card-fold-keep");
        h.classList.add("card-fold-head");
        h.tabIndex = 0;
        h.setAttribute("role", "button");
        const toggle = () => {
          const all = load();
          setFolded(card, !card.classList.contains("collapsed"), all);
          keep(all);
        };
        h.addEventListener("click", toggle);
        h.addEventListener("keydown", (e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            toggle();
          }
        });
        setFolded(card, folded.has(card.dataset.fold), folded);
      });
    };
    setup();
    const watch = new MutationObserver(setup);
    watch.observe(root, { childList: true, subtree: true });
    return () => watch.disconnect();
  }, [pageRef]);
}

/** Folds every card on the page, or opens them all. */
export function foldAll(root: HTMLElement | null, fold: boolean) {
  if (!root) return;
  const folded = load();
  root.querySelectorAll<HTMLElement>("section.card[data-fold]").forEach((card) => setFolded(card, fold, folded));
  keep(folded);
}

/** Opens the folded card an element is in (a search result, a quick link). */
export function unfold(el: Element | null | undefined) {
  const card = el?.closest<HTMLElement>("section.card.collapsed") ?? (el instanceof HTMLElement && el.matches("section.card.collapsed") ? el : null);
  if (!card) return;
  const folded = load();
  setFolded(card, false, folded);
  keep(folded);
}
