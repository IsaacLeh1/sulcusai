# Security policy

Version 1, 2026-11-03

## Supported versions

Only the newest release gets security fixes. SulcusAI updates itself by default, with signed update files; keep automatic updates on.

## Reporting a problem

Please report security problems privately, not in a public issue:

- **Preferred:** GitHub's private vulnerability reporting, under the repository's Security tab ("Report a vulnerability").
- **Or by email:** [security contact address].

Include what you found, the version, how to reproduce it and what an attacker could do with it. A proof of concept helps. Please don't include anyone's personal data.

## What happens next

- You'll get a reply within 7 days.
- Confirmed problems are fixed as fast as their severity calls for, and the fix ships as a normal signed update.
- Once the fix is out, the problem is described in the release notes, and you're credited if you'd like to be.
- Please give a fix 90 days before you publish details, or less if it's already being used against people.

## In scope

- The SulcusAI app and its local API.
- The relay code in `relay/`, including relays people run themselves.
- The update and engine files and how the app checks their signatures.
- The website at sulcusai.sseditor.com.

## Out of scope

- Relays, servers and groups other people run: report those to their operators.
- Third-party models, add-ons and the services people connect (report those to their makers).
- Problems that need someone who already controls the PC or the Windows account.
- Denial of service by sending huge amounts of traffic.

## Safe harbor

Good-faith research that follows this policy, avoids harming people and their data, and stops at proof is welcome, and the publisher won't pursue legal action against it. Test only on your own devices and accounts.
