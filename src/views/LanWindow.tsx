// SPDX-License-Identifier: AGPL-3.0-only
// Networks in a window of their own: from the tray, or the light networks app.
import { useEffect } from "react";
import { errorText } from "../api";
import { Toasts, useToasts } from "../components/Toasts";
import { Networks } from "./groups/Networks";
import "./groups/social.css";

export default function LanWindow() {
  const toasts = useToasts();
  useEffect(() => {
    const show = (e: Event) => toasts.push(errorText((e as CustomEvent<string>).detail), "error");
    window.addEventListener("sulcus:error", show);
    return () => window.removeEventListener("sulcus:error", show);
  }, [toasts.push]);
  return (
    <div className="people-window nets-window">
      <Networks toast={toasts.push} />
      <Toasts toasts={toasts.list} onDismiss={toasts.dismiss} />
    </div>
  );
}
