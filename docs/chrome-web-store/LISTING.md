# Chrome Web Store submission kit

Everything to paste into the developer dashboard (https://chrome.google.com/webstore/devconsole), in the order the dashboard asks for it. Chrome does not let any extension or automation tool script Web Store pages, so the forms are filled by hand.

Files in this folder:

- `screenshots/` four screenshots, 1280x800.
- `small-promo-tile.png` the 440x280 promo tile.
- The package to upload is `dist/adm-extension-<version>-store.zip`, made by `python3 packaging/build-extension.py --store`. It is the release package without the development `key`, because the Store assigns the extension its own ID.

## Before you start

1. A developer account with the one-time registration fee, and 2-step verification on the Google account.
2. A verified contact email in the dashboard's Account tab.

## 1. Upload

Items, then **New item**, then upload `adm-extension-0.5.0-store.zip`.

After the upload the address bar shows the item's ID: `.../devconsole/<publisher>/<item ID>/edit`. The item ID is 32 letters. Keep it: ADM must be told to trust it (see "After the first upload").

## 2. Store listing

| Field | Value |
|---|---|
| Name | ADM Download Manager (read from the package) |
| Summary | Sends your browser's downloads to the ADM app: a fast, lightweight download manager with multi-connection speed and resume. (read from the package) |
| Category | Productivity |
| Language | English |
| Icon | the package's 128x128 icon (read from the package) |
| Screenshots | upload the four files in `screenshots/` |
| Small promo tile | `small-promo-tile.png` |
| Homepage URL | https://github.com/Axenith/ADM (leave "Official URL" at None: it needs a verified domain) |
| Support URL | https://github.com/Axenith/ADM/issues |

**Description**

```
ADM Download Manager sends your browser's downloads to ADM, a fast and lightweight download manager for Windows and Linux.

This extension needs the free ADM app. Install it first from https://github.com/Axenith/ADM/releases/latest . Without the app the extension leaves your browser's downloads alone.

What it does
- Hands browser downloads to ADM, which splits each file across several connections, retries when the connection drops, and resumes where it stopped.
- Starts ADM for you when it is not running, then hands the download over. If ADM cannot be started, the browser downloads the file as usual, so a download is never lost.
- Lets you choose which file types and sizes it takes over, and which sites it leaves alone, in ADM's settings.
- Right-click any link and choose "Download with ADM".
- Grab links (Alt+Shift+G): collect the links on a page, or everything the page loads, filter them by type, words, wildcard or regular expression, and send them to ADM as one group.
- Browser monitoring (optional): lists the videos and audio a page loads, with a download button for each.
- Login cookies (optional): lets ADM download files that need you to be signed in.
- Private windows are never captured.

Privacy
The extension talks only to the ADM app on your own computer. It sends nothing to any server on the internet. The optional features are off until you allow them. Full policy: https://github.com/Axenith/ADM/blob/main/PRIVACY.md

ADM is open source (Apache-2.0): https://github.com/Axenith/ADM
```

## 3. Privacy practices

**Single purpose**

```
Send the browser's file downloads, selected media and chosen links to the ADM download manager app on the user's own computer, and start that app when it is needed.
```

**Permission justifications** (one box per permission)

| Permission | Justification |
|---|---|
| `downloads` | To notice a download the browser is about to make and, when ADM can take it, cancel the browser's copy and hand the link to ADM. |
| `nativeMessaging` | To start the ADM app when it is not running, through a small launcher program that ADM's installer registers. The browser only lets this extension talk to that launcher, and the launcher can only start ADM. |
| `contextMenus` | Adds the "Download with ADM" item to the right-click menu of links, videos and audio. |
| `storage` | Remembers the extension's settings (for example, whether to start ADM automatically) and keeps each tab's list of detected media in memory. |
| `alarms` | Checks once a minute whether ADM is running, to show it on the toolbar icon. |
| `activeTab` | Reads the links of the current page only when the user chooses "Grab links". |
| `scripting` | Runs the link collector in the current page when the user chooses "Grab links" (together with `activeTab`). |
| Host permission `http://127.0.0.1:8598/*` | Talks to the ADM app on the user's own computer. This is a local address, not a website. |
| Optional `cookies` | Only if the user turns on login cookies: passes the site's cookies for a download to ADM, so a file that needs sign-in can be downloaded. Sent only to ADM on this computer. |
| Optional `webRequest` | Only if the user turns on browser monitoring: reads response types and sizes to list videos and audio, and the headers of requests that become downloads so ADM can repeat them exactly. Nothing is blocked or changed. |
| Optional host permission, all sites | Needed together with `cookies` and `webRequest`, which work per site. Requested only when the user turns those features on. |

**Remote code**: No, I am not using remote code.

**Data usage**: the extension handles download addresses, cookies and request headers on the user's computer and passes them only to the ADM app on the same computer. It does not transmit anything to the developer or to any third party. Leave all data-type boxes unchecked, then tick all three certifications (no sale of user data, no use unrelated to the single purpose, no creditworthiness or lending use).

**Privacy policy URL**: https://github.com/Axenith/ADM/blob/main/PRIVACY.md

## 4. Distribution

- Visibility: Public.
- Regions: all.
- Pricing: free.

## 5. Notes for the reviewer (Test instructions)

```
The extension needs the free ADM desktop app, which is open source. To test:

1. Install ADM from https://github.com/Axenith/ADM/releases/latest (Windows: adm-<version>-windows-x64-setup.exe; Linux: the tar.gz and install.sh). Start ADM once from the application menu so it registers with the browser, then close its window (it stays in the tray).
2. Install this extension and open any page with a download link, for example https://github.com/Axenith/ADM/releases/latest .
3. Click a download link, or right-click it and choose "Download with ADM". ADM opens its New download window with the file. If ADM was fully quit (tray menu, Quit), the extension starts it first.
4. Click the extension's toolbar icon for the media list and "Grab all links for this page".

Without the ADM app the extension does nothing and the browser downloads files normally. The optional features (login cookies, browser monitoring) are enabled from the extension's options page and show the browser's own permission prompt.
```

## After the first upload

The Store gives the extension a new ID, different from the one used when the extension is loaded unpacked. ADM's browser launcher only accepts IDs it knows, so:

1. Send the 32-letter item ID to the maintainer (or add it to `EXTENSION_IDS` in `src-tauri/src/native_host.rs`).
2. Release a new ADM version that includes it. Until then the Store version works for everything except starting ADM automatically.
3. Publish the item for review from the dashboard (**Submit for review**).

Reviews of extensions with `nativeMessaging` and optional access to all sites can take longer than usual. Reply to reviewer questions from the dashboard.
