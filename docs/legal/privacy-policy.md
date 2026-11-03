# SulcusAI Privacy Policy

Version 1, 2026-11-03

SulcusAI is a desktop app that runs AI models on your own computer. It is open source, so anyone can check what this policy says against the code. "The publisher" means the developer who publishes SulcusAI, or a company that later takes over publishing it.

## The short version

- There are no SulcusAI accounts, no analytics, no advertising and no tracking.
- Your chats, files, mail, calendars and everything else you make stay on your computer, encrypted.
- The AI runs on your computer by default. Nothing goes to an online AI service unless you turn that on.
- Your data is never sold, never used for advertising and never used to train AI models. The publisher never receives it.
- Every request that leaves your computer is recorded in the app's activity log on your computer.

## What stays on your computer

Everything you create or connect is stored in the app's data folder on your computer: chats, memories, notes, tasks, documents, recordings and transcripts, pictures, videos, songs, voices, models and copies of the mail and calendar events you connect.

- Chats, your profile, sign-in tokens, mail, calendar data and other personal data are encrypted with AES-256-GCM.
- The key is sealed to your account on this computer and, if you turn on app lock, to your PIN or passphrase.
- The password vault is encrypted separately with your master password.

## What goes over the internet, and only when you ask

| What | Where it goes | When |
|---|---|---|
| Models and engines you choose | Hugging Face, GitHub | When you install them |
| Update checks and updates | GitHub | Automatic updates are on by default and are a switch |
| Web searches and pages | The sites and search services you choose | When web access is on |
| Mail and calendars | Your own mail and calendar providers | For accounts you connect |
| Online AI providers | The provider you choose, with your own key | Only if you turn on Cloud |
| Live data (weather, maps, prices, Wikipedia) | The data service for that request | When you ask for it with web access on |
| The private connection | Tor or your own proxy | If you turn it on |

When you use an online AI provider, the text the assistant needs for a reply is sent to that provider, under its own terms and privacy policy. That can include parts of your mail, calendar or files if you ask about them. SulcusAI can mask personal details such as email addresses and phone numbers before sending; this is on by default.

## Connections between people and devices

- **Sync between your own devices** is end-to-end encrypted with a key only your devices share.
- **Groups, calls, file sharing and the virtual LAN** connect people's devices directly, or through relays that people run themselves. Whoever runs a relay can see that two addresses talk, when, and how much, but not what is said. People you connect to directly can see your IP address unless you use the address shield's relay-only option.
- **Relays and servers run by other people** are theirs. Their privacy practices are theirs, not the publisher's.

## The publisher's relay, until it is retired

For now, the publisher runs one relay on Cloudflare's free plan. It is used for:

- sync between your own PCs when they aren't on the same network: it holds sealed messages it can't read until your other PC collects them, and erases a mailbox untouched for 30 days;
- the phone link to your own PC: it passes the app's own files and sealed requests it can't read, and erases a link whose PC hasn't connected for 90 days;
- People connections without a VPN: it passes sealed bytes between two apps and stores nothing;
- the Bug / Feedback button: it keeps only the text you write, plus a reply address and the version number if you choose to add them. Nothing about your computer, account or chats is sent. To limit abuse, it counts messages per sender with a hash of the sender's address that changes every hour and is then deleted.

Cloudflare, which carries this traffic, can see IP addresses and timing as any network provider can, under its own privacy policy. This section will be removed when the relay is retired, and you'll be asked to accept the new version.

## Google and Microsoft accounts

If you sign in with Google or Microsoft for mail or calendar, the sign-in page opens in your own browser and SulcusAI never sees your password. With your permission, the app gets a token that allows only what the feature needs:

- **Google:** Gmail (`https://mail.google.com/`) to read, search, sort and send mail you write or approve; Google Calendar (`.../auth/calendar`) to show your events and create or delete the ones you choose; and your email address (`openid`, `email`) to label the account.
- **Microsoft:** `IMAP.AccessAsUser.All` and `SMTP.Send` for mail, `Calendars.ReadWrite` for calendars, and `openid`, `email`, `offline_access` for the sign-in.

Mail and events are copied into the encrypted store on your computer so you can read and search them offline. This data is used only to provide these features to you, and is shared with no one except an online AI provider you choose, as described above.

**Limited Use.** SulcusAI's use and transfer of information received from Google APIs to any other app will adhere to the Google API Services User Data Policy, including the Limited Use requirements. Google user data is never used to develop, improve or train generalized AI or machine-learning models.

You can revoke access at any time at myaccount.google.com/permissions (Google) or account.live.com/consent/Manage (Microsoft).

## Your choices

- **See it:** the activity log lists every request that left your computer.
- **Export it:** Settings, "Export everything" writes your chats and notes as readable files and a password-protected backup of everything else.
- **Delete it:** removing an account deletes its token and copied mail or events. Settings, "Delete everything" erases the app's data folder the next time the app starts. Uninstalling and deleting the data folder does the same.
- **Turn things off:** almost every feature is a switch, and starts off.

## Children

SulcusAI is not meant for children under 13. People from 13 to 17 need a parent or guardian to agree to the Terms of Use, and social features start with teen settings. Because nothing reaches the publisher, the publisher holds no information about anyone, children included.

## Changes

If this policy changes, the new version gets a new number and date, and the app asks you to read it again when the change matters.

## Contact

Privacy questions: [legal contact address], or open an issue on GitHub (don't post personal details there).
