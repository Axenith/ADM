import { ADM_HOME, cookieAccessEverywhere, isAlive, monitoringAccess, requestCookieAccess, requestMonitoringAccess } from "./adm.js";

const $ = (id) => document.getElementById(id);

if (ADM_HOME) {
  $("home").href = ADM_HOME;
  $("home").hidden = false;
}

async function drawStatus() {
  const alive = await isAlive();
  $("status").className = "status " + (alive ? "ok" : "off");
  $("status").querySelector("span").textContent = alive ? "ADM is running and connected" : "ADM is not running yet";
}
drawStatus();
setInterval(drawStatus, 3000);

async function drawCookies() {
  const on = await cookieAccessEverywhere();
  $("allow").hidden = on;
  $("result").textContent = on ? "Login cookies are allowed. You can turn this off from the toolbar popup." : "";
  $("result").className = "result" + (on ? " ok" : "");
}
drawCookies();

$("allow").addEventListener("click", async () => {
  let granted = false;
  try { granted = await requestCookieAccess(); } catch { /* dismissed */ }
  if (!granted) $("result").textContent = "Not allowed. Downloads still work; ones that need you to be signed in may fail.";
  else drawCookies();
});

async function drawMonitoring() {
  const on = await monitoringAccess();
  $("allow-mon").hidden = on;
  $("mon-result").textContent = on ? "Browser monitoring is on. You can turn it off from the toolbar popup." : "";
  $("mon-result").className = "result" + (on ? " ok" : "");
}
drawMonitoring();

$("allow-mon").addEventListener("click", async () => {
  let granted = false;
  try { granted = await requestMonitoringAccess(); } catch { /* dismissed */ }
  if (!granted) $("mon-result").textContent = "Not allowed. Downloads still work; the toolbar just won't list videos.";
  else { await chrome.storage.local.set({ monitoring: true }); drawMonitoring(); }
});
if (location.hash === "#monitoring") $("monitoring").scrollIntoView();
