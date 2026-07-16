"use strict";

const REFRESH_SECONDS = 10;
const rows = [...document.querySelectorAll("[data-endpoint]")];
const countdown = document.querySelector("#refresh-countdown");
const lastChecked = document.querySelector("#last-checked");
const installCommand = document.querySelector("#install-command");
const copyCommand = document.querySelector("#copy-command");

let nextRefreshAt = Date.now();
let refreshing = false;

const command = [
  `curl -fsSL ${window.location.origin}/downloads/jit-linux-x86_64 -o jit`,
  "chmod +x jit",
  "./jit --version",
].join("\n");
installCommand.textContent = command;

copyCommand.addEventListener("click", async () => {
  try {
    await navigator.clipboard.writeText(command);
    showCopyResult("Copied");
  } catch (_) {
    const range = document.createRange();
    range.selectNodeContents(installCommand);
    window.getSelection().removeAllRanges();
    window.getSelection().addRange(range);
    showCopyResult("Selected");
  }
});

function showCopyResult(label) {
  copyCommand.textContent = label;
  window.setTimeout(() => { copyCommand.textContent = "Copy"; }, 1600);
}

async function probe(row) {
  const endpoint = row.dataset.endpoint;
  const startedAt = performance.now();
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), 4000);

  try {
    const response = await fetch(endpoint, {
      cache: "no-store",
      credentials: "same-origin",
      signal: controller.signal,
    });
    const elapsed = Math.round(performance.now() - startedAt);
    const body = await response.json().catch(() => ({}));
    const healthy = response.ok && ["ok", "ready"].includes(body.status);
    updateRow(row, healthy ? "ok" : "error", healthy ? statusLabel(endpoint) : "Not ready", `${response.status} · ${elapsed} ms`);
  } catch (_) {
    updateRow(row, "error", "Unreachable", "No response");
  } finally {
    window.clearTimeout(timeout);
  }
}

function statusLabel(endpoint) {
  return endpoint === "/readyz" ? "Ready" : "Healthy";
}

function updateRow(row, state, label, detail) {
  row.classList.remove("pending", "ok", "error");
  row.classList.add(state);
  row.querySelector(".status-value strong").textContent = label;
  row.querySelector(".status-value span").textContent = detail;
}

async function refresh() {
  if (refreshing) return;
  refreshing = true;
  countdown.textContent = "Refreshing…";
  await Promise.all(rows.map(probe));
  lastChecked.textContent = `Checked ${new Date().toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}`;
  nextRefreshAt = Date.now() + REFRESH_SECONDS * 1000;
  refreshing = false;
  updateCountdown();
}

function updateCountdown() {
  if (refreshing) return;
  const seconds = Math.max(0, Math.ceil((nextRefreshAt - Date.now()) / 1000));
  countdown.textContent = `Refresh in ${seconds}s`;
  if (seconds === 0) refresh();
}

window.setInterval(updateCountdown, 250);
document.addEventListener("visibilitychange", () => {
  if (!document.hidden) refresh();
});

refresh();
