# Channel management and subscription import (design, future)

Goal: make adding/removing channels easy, ideally from the user's YouTube
subscriptions via Google sign-in.

## Today

`channels.conf` (one handle/URL/ID per line, optional `|name`), plus
add/remove entries in the main menu. The add-on's settings are only cache
tuning and the quality/engine settings.

## Plan, in order of value for effort

1. **Manage channels dialog** from add-on settings (an `action` setting that
   runs a script): multi-select to remove many, rename, reorder, add by
   search (`yt-dlp ytsearch` filtered to channels) or by pasting a URL/handle.
   No auth, small.
2. **Import subscriptions without auth**: Google Takeout
   (`subscriptions.csv`: channel id, URL, title) or an OPML/CSV file path in
   settings. Robust, no Google project, works offline.
3. **Live subscription sync via Google sign-in (optional)**: OAuth 2.0
   *device flow* (the TV/limited-input flow: Kodi shows a code, the user
   approves on a phone), then `subscriptions.list?mine=true` (YouTube Data
   API v3, read-only scope).

## OAuth findings

- Google documents the device flow for TVs/limited-input devices
  ([docs](https://developers.google.com/accounts/docs/OAuth2ForDevices)).
  Search results did **not** confirm that the YouTube read-only scope is on
  the device flow's allowed-scopes list; **check that list before building**
  (Hypothesis: it is allowed).
- It needs an OAuth client the user creates in their own Google Cloud
  project (client ID and secret entered in settings; never committed to the
  repo, never shipped in the add-on).
- A client in "Testing" publishing status gets refresh tokens that **expire
  after 7 days** ([Google, Data Portability OAuth guide](https://developers.google.com/data-portability/user-guide/configure-oauth),
  [YouTube API thread](https://discuss.google.dev/t/oauth2-refresh-token-expiration-and-youtube-api-v3/160874)).
  Longer lifetimes need an "In production" client, which for sensitive scopes
  means Google verification. For a personal install this makes live sync
  awkward: re-link weekly, or verify the app.
- Store tokens in the add-on profile (`addon_data`), not in the settings
  XML that users may share.

## Alternative: cookies with yt-dlp

yt-dlp can read the subscriptions feed with exported cookies, no API
project. Fragile (page changes), puts account cookies on the TV box, and risks
account flags. Not recommended.

## Recommendation

Ship 1 and 2 first (immediate value, no credentials). Offer 3 as an opt-in
"Sync subscriptions" action with a clear note about the 7-day limit and a
one-shot "import now" mode (list once, then drop the token) that sidesteps
refresh-token expiry.
