# ADM browser extension

Sends your browser's downloads to ADM. Manifest V3, plain JavaScript, no build step (about 9 KB without icons).
Works in Brave, Chrome and other Chromium browsers. Firefox is not supported yet.

## Install (Brave / Chrome)

1. Open `brave://extensions` (or `chrome://extensions`) and turn on **Developer mode**.
2. Click **Load unpacked** and choose this `extension` folder.
3. Start ADM. The toolbar icon shows a red **!** while ADM is not running.

## What it does

- **Browser downloads** go to ADM instead. ADM shows its **New download** window (file name, folder, connections,
  free space), so you decide before anything starts. The browser's own copy is cancelled and removed.
- **Videos on the page** (opt-in **Browser monitoring**): the toolbar icon counts the video and audio files a page
  loads, and the popup lists them with their size and a download button each ("More formats" shows the rest,
  "Clear list" empties it). Streams made of many small pieces (HLS/DASH) are not listed: ADM cannot assemble those yet.
- **Grab all links for this page…**: lists the page's links by type (video, audio, archives, documents, programs,
  images, pages) with a filter box; select some and send them to ADM as one group (one window, not one per link).
- **Right-click a link, video or audio, then "Download with ADM".**
- **Never loses a download.** If ADM is not running, or refuses a link, the browser downloads it as usual.
- **"Don't capture downloads from this site"** (in ADM's New download window, or its Settings) makes the extension
  leave that site's downloads to the browser from then on.
- The toolbar icon is in colour while ADM is running and grey while it is not. **Private windows are never captured.**

It only talks to ADM on `127.0.0.1:8598`, and ADM only answers requests that come from a browser extension, so web
pages cannot reach it. Nothing is sent to any server on the internet.

## Permissions

Installing asks for very little: `downloads` (to take over a download), `contextMenus`, `storage`, `alarms`,
`activeTab` and `scripting` (to read a page's links only when you press "Grab all links"), and access to ADM on
`127.0.0.1:8598`.

Two features are **opt-in**, each behind a button (on the first-run page, in the toolbar popup, or in the extension's
options). Chrome then asks for access to all sites; until you say yes the extension does not have it:

- **Login cookies** lets ADM download files that need you to be signed in, by passing it the cookies the site would see
  (never anywhere else).
- **Browser monitoring** reads each response's type and size to find videos. It never blocks or changes a request, and
  the list lives in memory for that tab only.

Both can be turned off again from the options page.

## Things to know

- Only one extension can take over a browser's downloads. **Turn off XDM's extension** (and any other download
  manager extension) while this one is on, or they will fight over each download.
- A download that arrives while ADM's window is closed opens the window and waits there. Closing ADM first does not lose
  it: waiting links are saved and come back on the next start.

## Tests

```
node --test extension/test/extension.test.mjs   # mocked browser + mock ADM
node extension/test/browser-e2e.mjs              # the extension in a REAL headless Brave/Chromium (needs `brave`)
node extension/test/real-adm.mjs                 # against a running ADM; downloads a 3 MB test file into ~/Downloads
```
