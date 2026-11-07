// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import type { MemberRow } from "./Networks";

describe("Networks online peer formatting", () => {
  const members: MemberRow[] = [
    { peer: "peer-me", name: "Me", address: "26.14.88.100", admin: true, me: true, online: true, ping: 0, device: "dev-me", relayed: false, why: "" },
    { peer: "peer-alice", name: "Alice", address: "26.14.88.101", admin: false, me: false, online: true, ping: 24, device: "dev-alice", relayed: false, why: "" },
    { peer: "peer-bob", name: "Bob", address: "26.14.88.102", admin: false, me: false, online: true, ping: 48, device: "dev-bob", relayed: false, why: "" },
    { peer: "peer-charlie", name: "Charlie", address: "26.14.88.103", admin: false, me: false, online: false, ping: null, device: "dev-charlie", relayed: false, why: "" },
  ];

  it("filters connected peers excluding self", () => {
    const onlinePeers = members.filter((m) => m.online && !m.me);
    expect(onlinePeers.map((p) => p.name)).toEqual(["Alice", "Bob"]);
  });

  it("identifies online vs offline members correctly", () => {
    const online = members.filter((m) => m.online);
    const offline = members.filter((m) => !m.online);
    expect(online).toHaveLength(3);
    expect(offline).toHaveLength(1);
    expect(offline[0].name).toBe("Charlie");
  });

  it("formats peer summary labels with truncation when over limit", () => {
    const formatPeers = (list: MemberRow[], max = 2) => {
      const names = list.map((p) => p.name).slice(0, max).join(", ");
      const extra = list.length > max ? ` +${list.length - max}` : "";
      return `${names}${extra}`;
    };

    const onlinePeers = members.filter((m) => m.online && !m.me);
    expect(formatPeers(onlinePeers, 1)).toBe("Alice +1");
    expect(formatPeers(onlinePeers, 2)).toBe("Alice, Bob");
  });
});
