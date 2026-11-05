# Hosting your own relay

The relay runs on any server you control, for example an Oracle Cloud
Always Free machine. SulcusAI's Settings > Network > Host your own relay
writes the setup for you; this page explains the ports.

## Ports to open (Oracle: your subnet's Security List, ingress from 0.0.0.0/0)

| Port | Protocol | Needed for |
| --- | --- | --- |
| 4001 (or your `--port`) | TCP and UDP | The relay itself. Always. |
| 443 | TCP | The phone bridge (`--phone-port 443`). Only if you use it. |
| 80 | TCP | Let's Encrypt's check (`--acme`). Only with the phone bridge. |

Open the same ports in the server's own firewall (the setup does this for
the relay port with iptables).

## The phone bridge and its certificate

    sulcus-relay run --phone-port 443 --acme <your domain or public IP> [--acme-email you@example.org]

The relay gets its certificate from Let's Encrypt and renews it on its own.
A bare IP address works: Let's Encrypt then signs a certificate that lasts
about 6 days, and the relay renews it every 3.

Let's Encrypt checks you control the address by asking for a file over
port 80, so port 80 must be open. What the relay does to keep that safe:

- Port 80 listens only while a certificate is being fetched, then closes.
- It answers only the exact file Let's Encrypt asked for at that moment.
  Every other request gets a plain redirect to `https://<your address>/`,
  with no version or server name in it. Request bodies are never read.
- Requests over 2 KB, clients slower than 5 seconds, more than 4
  connections from one address, 64 in all, or 30 a minute from one
  address are cut off.
- HTTPS uses TLS 1.2 and 1.3 only, tells browsers to use HTTPS only (HSTS),
  closes connections idle for a minute, and limits connections per address.
- The certificate, its key and the Let's Encrypt account are kept in
  `<dir>/acme/<address>/`, readable by the relay's user only. A new
  certificate replaces the old one only once it is complete and checked;
  if renewal fails, the old one keeps working and the relay tries again
  after 5 minutes, then 10, 20 and so on, up to every 3 hours.

Ports below 1024 need permission on Linux. If the relay runs as its own
user, add `AmbientCapabilities=CAP_NET_BIND_SERVICE` under `[Service]` in
`/etc/systemd/system/sulcus-relay.service`, then
`sudo systemctl daemon-reload && sudo systemctl restart sulcus-relay`.

If you already run a web server with a certificate, use
`--phone-port 8080 --behind-proxy` instead and leave port 80 to it.
