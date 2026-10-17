# youtube-on-kodi-nix

Notes and research for the NixOS packaging and the maintained fork of the
Youtube-On-Kodi add-on. These pages are kept in the repository under `docs/wiki/` (the GitHub wiki
could not be pushed to yet) and are written so they can be copied into it
unchanged. Source of truth for the code is the repository; these
pages record *why* things are the way they are.

| Page | Status |
|------|--------|
| [Seek and resume investigation](Seek-and-resume-investigation) | Root causes found and fixed; live-YouTube confirmation pending |
| [Automated testing](Automated-testing) | Implemented (`nix run .#e2e`) |
| [Channels as TV shows](Channels-as-TV-shows) | Research / design |
| [Unified "next up" view](Unified-next-up-view) | Research / design |
| [Channel management and subscriptions](Channel-management-and-subscriptions) | Research / design |

Conventions used in these pages: **Verified** means reproduced here with
Kodi 21.3 and the e2e harness. **Hypothesis** means reasoned from docs or
memory and not yet tested.
