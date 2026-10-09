// SPDX-License-Identifier: AGPL-3.0-only
import { Component, type ReactNode } from "react";

/** Catches a rendering crash so the window shows what happened instead of going blank. */
export class CrashScreen extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error) {
    console.error(error);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    return (
      <div className="onboarding">
        <div className="onboarding-card">
          <h1>Something went wrong</h1>
          <p>This screen hit an error. Your chats and settings are safe. Reloading usually fixes it.</p>
          <pre className="crash-detail">{error.message}</pre>
          <div className="onboarding-nav">
            <button className="btn ghost" onClick={() => this.setState({ error: null })}>Try again</button>
            <button className="btn primary" onClick={() => window.location.reload()}>Reload</button>
          </div>
        </div>
      </div>
    );
  }
}
