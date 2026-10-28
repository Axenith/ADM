import { admState, cookieAccessEverywhere, dropCookieAccess, dropMonitoringAccess, monitoringAccess, requestCookieAccess, requestMonitoringAccess, settings } from "./adm.js";

const $ = (id) => document.getElementById(id);

async function drawStatus() {
  const alive = (await admState()) !== null;
  $("status").className = "status " + (alive ? "ok" : "off");
  $("status").querySelector("span").textContent = alive ? "ADM is running and connected" : "ADM is not running";
}

/** One permission-backed feature: its state, and a button that asks for it (this page is a normal tab, so Chrome's prompt works). */
function feature(prefix, has, ask, drop, after = async () => {}) {
  async function draw() {
    const on = await has();
    $(`${prefix}-state`).textContent = on ? "On" : "Off";
    $(`${prefix}-btn`).textContent = on ? "Turn off" : "Turn on";
    $(`${prefix}-btn`).onclick = async () => {
      if (on) await drop();
      else {
        let granted = false;
        try { granted = await ask(); } catch { /* dismissed */ }
        $(`${prefix}-result`).textContent = granted ? "" : "Not allowed. You can try again any time.";
        if (granted) await after();
      }
      draw();
    };
  }
  draw();
}

(async () => {
  const s = await settings();
  $("enabled").checked = s.enabled;
  $("min").value = s.minSizeMB;
  $("autostart").checked = s.autoStart;
  $("autostart").addEventListener("change", () => chrome.storage.local.set({ autoStart: $("autostart").checked }));
  $("enabled").addEventListener("change", () => chrome.storage.local.set({ enabled: $("enabled").checked }));
  $("min").addEventListener("change", () => chrome.storage.local.set({ minSizeMB: Math.max(0, Number($("min").value) || 0) }));
  feature("cookies", cookieAccessEverywhere, requestCookieAccess, dropCookieAccess);
  feature("mon", monitoringAccess, requestMonitoringAccess, dropMonitoringAccess, () => chrome.storage.local.set({ monitoring: true }));
  drawStatus();
  setInterval(drawStatus, 3000);
})();
