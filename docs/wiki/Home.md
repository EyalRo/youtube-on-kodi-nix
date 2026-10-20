# youtube-on-kodi-nix

Notes and research for the NixOS packaging and the maintained fork of the
Youtube-On-Kodi add-on. These pages live in the repository under `docs/wiki/` and are mirrored to the GitHub wiki. Source of truth for the code is the repository; these
pages record *why* things are the way they are.

| Page | Status |
|------|--------|
| [Seek and resume investigation](Seek-and-resume-investigation) | Root causes found and fixed; live-YouTube confirmation pending |
| [Automated testing](Automated-testing) | Implemented (`nix run .#e2e`) |
| [Channels as TV shows](Channels-as-TV-shows) | Research / design |
| [Unified "next up" view](Unified-next-up-view) | Research / design |
| [Games](Games) | Scanner implemented; launch verified in e2e |
| [Home rows](Home-rows) | Implemented; verified in e2e |
| [Media appliance](Media-appliance) | Built and verified in VMs; NUC hardware gates open |
| [Rust appliance feasibility](Rust-appliance-feasibility) | Research: what exists, what is missing |
| [Channel management and subscriptions](Channel-management-and-subscriptions) | Research / design |

Conventions used in these pages: **Verified** means reproduced here with
Kodi 21.3 and the e2e harness. **Hypothesis** means reasoned from docs or
memory and not yet tested.
