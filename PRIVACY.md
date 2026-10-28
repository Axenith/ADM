# Privacy policy

ADM Download Manager (the ADM app and its browser extension) does not collect, store on any server, sell or share personal data. It has no accounts, no analytics and no advertising, and it makes no network requests of its own to any service run by its authors.

## What the browser extension does

The extension passes your browser's downloads to the ADM app running on your own computer. It talks to ADM only through `127.0.0.1` (your own machine) and through the browser's native messaging, which it uses to start ADM when ADM is not running. Nothing the extension reads leaves your computer through the extension.

To do this it handles, on your computer only:

- The address of a download, the page it came from, its file name and size, so ADM can fetch it.
- Cookies and request headers for a download, only if you allow it (see Permissions). They are passed to ADM so a download that needs you to be signed in can work.
- The web addresses and content types of media a page loads, only if you turn on browser monitoring. The list is kept in memory for the tab and is discarded when the tab closes.
- A few settings (for example, whether to start ADM automatically), stored in the browser's extension storage.

## What the ADM app does

ADM downloads the files you ask it to download, from the servers that host them, and saves them on your computer. Those servers see ordinary download requests from your IP address, as they would from your browser. ADM keeps its download list and settings in a folder in your user profile. The list can include cookies for downloads that are waiting or paused, and it is readable only by your user account.

## Permissions the extension asks for

- `downloads`: to take over a browser download and hand it to ADM.
- `nativeMessaging`: to start the ADM app when it is not running. It can start only ADM.
- `contextMenus`: the "Download with ADM" menu item.
- `storage`: to remember the extension's settings and the per-tab media list.
- `alarms`: to refresh the toolbar icon, which shows whether ADM is running.
- `activeTab` and `scripting`: to read a page's links, only when you choose "Grab links".
- Access to `http://127.0.0.1:8598/*`: to talk to ADM on your computer.
- Optional, asked for only when you turn a feature on: `cookies` and access to all sites (login cookies), and `webRequest` (browser monitoring and exact replay of downloads).

## Your choices

You can turn off taking over downloads, starting ADM automatically, browser monitoring and login cookies in the extension's options, and remove the extension at any time. Uninstalling ADM does not delete your downloaded files.

## Changes and contact

If this policy changes, the new version is published in this file, in the project repository: https://github.com/Axenith/ADM. Questions can be raised as an issue there.
