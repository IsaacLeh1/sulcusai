# sulcus-relay-spike

A throwaway check, before phase 5's network layer is built, that rust-libp2p's Circuit Relay v2 and DCUtR (hole punching) work together the way the design needs. It is its own Cargo project: not part of the app's build, workspace or lockfile. It goes away when the real `sulcus-net` crate lands.

The phase 5 design (`phase5-design.md`) is kept with the project notes, outside this repo; this folder is the part of it that runs.

## What it does

Three libp2p nodes on 127.0.0.1, in one process or three:

- a **relay** (Circuit Relay v2 server, identify, ping)
- a **listener** that reserves a slot on the relay and becomes reachable through it
- a **dialer** that reaches the listener through the relay, after which DCUtR tries to replace the relayed connection with a direct one

Transports are TCP and QUIC, with Noise and yamux. Every address is loopback and nothing resolves names, so it never reaches past this PC.

## Running it

Use a target folder outside the repo (`CARGO_TARGET_DIR`).

```
cargo run --release -- demo                    # all three in one process, relay over TCP, then DCUtR
cargo run --release -- demo --quic             # the same with the relay reached over QUIC
cargo run --release -- demo --confirm          # own addresses confirmed up front: DCUtR finds nothing to offer
cargo run --release -- demo --relay-only       # DCUtR off; push 1 MiB through the relay instead
cargo run --release -- rates 30                # how often TCP and QUIC end up direct
cargo test --release                           # the same demos as tests

# or as three processes
cargo run --release -- relay 4801
cargo run --release -- listen /ip4/127.0.0.1/tcp/4801
cargo run --release -- dial /ip4/127.0.0.1/tcp/4801
```

## What loopback can't show

Hole punching exists to get through routers. On 127.0.0.1 there is no router: no address translation, no filtering of unsolicited packets, no symmetric NAT changing ports per destination, no timing race between the two sides' packets, no firewall (Windows Firewall does not filter loopback) and no real latency. So a successful upgrade here proves the protocol plumbing (addresses exchanged over the relay, both sides dialing, the direct connection replacing the relayed one), not that it works through real routers. That needs the NAT lab and real-network tests in the design's test plan.

## Results (2026-11-03, Windows 11, Rust 1.99.1)

- libp2p 0.57.1 with `tokio, tcp, quic, noise, yamux, relay, dcutr, identify, ping, macros, ed25519`, plus `libp2p-stream` 0.5.0-alpha. 195 crates compiled; `cargo deny` clean (advisories, licences, sources).
- Release build (thin LTO, stripped, 2 jobs): 3 min 15 s cold, about a minute after a change. Binary 8.3 MB; `libp2p-quic` brings in aws-lc-rs, which the app doesn't use today.
- The relay, a reservation and a relayed connection worked every run. DCUtR then made a direct connection over TCP and over QUIC, a few milliseconds after the relayed one.
- Surprises:
  - Addresses confirmed before identify reports them leave DCUtR nothing to offer (it only uses candidates), so the punch fails.
  - Windows sometimes refuses the TCP simultaneous open (`os error 10048`); usually the other side's dial still gets through, occasionally neither does. QUIC had no such failures.
  - Both sides dial, so each ends with two direct connections, and the side whose dial lost the race may never get DCUtR's result.
  - App streams work over a relayed connection, but the relay's default limit (128 KiB, 2 minutes per circuit) cuts them, and the receiving side sees a clean end of stream rather than an error.
  - The relay needs a confirmed external address before it can take reservations.
