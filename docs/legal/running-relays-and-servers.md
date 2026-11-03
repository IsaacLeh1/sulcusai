# Running a relay or server

Version 1, 2026-11-03

SulcusAI has no central servers for people's data. Groups connect through relays that people run on their own PCs, a Raspberry Pi, a spare desktop or their own free cloud account. If you run one, this guide is for you. It isn't legal advice: the law where you live decides your duties.

## You are the operator

When you run a relay or server, own a group, create a virtual network or host a share, you are its operator. The publisher of SulcusAI has no access to it, no knowledge of it and no control over it. You decide who may use it, and you are responsible for it, as the Terms of Use say.

## What a relay sees

A SulcusAI relay passes sealed traffic it can't read. It does see:

- the IP addresses of the devices that connect to it;
- when they connect, and how much they send;
- the room or mailbox names they use (made from secrets only the two ends share, so they don't say who is talking).

Treat that as personal data. Keep no logs you don't need, and delete what you keep as soon as you can.

## Before you open it to others

1. **Decide who it's for.** A relay for your own devices or your family is very different from one open to strangers. Start closed: an invite or key for each person.
2. **Keep the limits on.** SulcusAI's relay limits how much each room passes a minute and an hour, so it can't become anyone's free pipe. Don't raise them for people you don't know.
3. **Know your connection.** Traffic through your relay leaves from your address. Check that your internet provider and your cloud plan allow relaying, and stay inside any free tier (SulcusAI's own-cloud option caps traffic for you).
4. **Publish a contact.** If people outside your household use it, give them a way to reach you about abuse, copyright or legal notices.
5. **Keep it up to date.** Turn on automatic updates. An old relay can have known holes.

## If you change the relay's code

SulcusAI is under the AGPL. If you run a changed version of the relay for other people, you must offer them its source code (AGPL section 13). Use your own name for it: the SulcusAI name and logo can't be used on a changed version (see TRADEMARKS.md).

## Notices and requests

Because only you can act on your relay, notices about it come to you.

- **Copyright notices:** if your relay or share holds files (a mailbox, a share), remove what a valid notice identifies. A relay that only passes sealed traffic holds nothing to remove; say so.
- **Law-enforcement requests:** you may be asked for logs or connection details. What you must give depends on where you are. You can't hand over what you didn't keep.
- **Abuse reports:** remove people who misuse your relay, and change the keys so they can't come back.
- **Child sexual abuse material:** if you learn of it, report it at once (see "Reporting" in the Community Guidelines). In the US, providers who learn of it have a legal duty to report it to NCMEC. Don't copy, open or share it, even to report it.

## Your group's rules

If you own or moderate a group, the Community Guidelines are the floor. Write your own rules on top, mark adult groups 18+, act on members' reports, and remove anything illegal as soon as you know of it.

## Shutting it down

You can stop a relay at any time. Tell the people who use it first if you can, so their apps can switch to another.
