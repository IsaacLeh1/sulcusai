// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import { proxyLines } from "./shield";

describe("proxyLines", () => {
  it("takes one proxy per line or comma and drops blanks", () => {
    expect(proxyLines(" 127.0.0.1:1080 \n\nsocks5://a:b@host:1080, http://10.0.0.2:3128\n")).toEqual(["127.0.0.1:1080", "socks5://a:b@host:1080", "http://10.0.0.2:3128"]);
    expect(proxyLines("   ")).toEqual([]);
  });
});
