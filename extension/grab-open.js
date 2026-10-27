import { collectLinks } from "./grab-lib.js";

/**
 * Reads the links and resources of a tab and opens the Grab page with them. Used by the popup and by the keyboard
 * shortcut. It runs only because the user asked (the `activeTab` permission), and throws on pages the browser
 * keeps off limits.
 */
export async function openGrab(tabId) {
  const [found] = await chrome.scripting.executeScript({ target: { tabId }, func: collectLinks });
  await chrome.storage.session.set({ grab: { ...found.result, tabId, at: Date.now() } });
  await chrome.tabs.create({ url: chrome.runtime.getURL("grab.html") });
}
