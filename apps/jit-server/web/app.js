"use strict";

const state = { csrf: null, expiresAt: null, pollTimer: null, toolsController: null };
const $ = (selector) => document.querySelector(selector);

function element(tag, options = {}, children = []) {
  const node = document.createElement(tag);
  if (options.className) node.className = options.className;
  if (options.text !== undefined) node.textContent = String(options.text);
  if (options.type) node.type = options.type;
  if (options.href) node.href = options.href;
  if (options.value !== undefined) node.value = options.value;
  if (options.name) node.name = options.name;
  if (options.placeholder) node.placeholder = options.placeholder;
  if (options.required) node.required = true;
  for (const child of Array.isArray(children) ? children : [children]) {
    if (child !== null && child !== undefined) node.append(child);
  }
  return node;
}

function clear(node) { node.replaceChildren(); }
function badge(value) { return element("span", { className: `badge ${value}`, text: statusText(value) }); }
function codeBlock(value) { return element("pre", { text: value ?? "" }); }
function formatTime(value) {
  if (!value) return "—";
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString("zh-CN");
}
function statusText(value) {
  return ({ queued: "排队中", running: "处理中", ready: "已就绪", not_ready: "未就绪", failed: "失败", rejected: "已拒绝", revoked: "已撤销", deprecated: "已弃用", draft: "草稿", contract_ready: "契约就绪", synthesizing: "合成中", building: "构建中", validating: "验证中", repairing: "修复中", complete: "完成" })[value] || value;
}
function showToast(message) {
  const toast = $("#toast");
  toast.textContent = message;
  toast.classList.remove("hidden");
  window.setTimeout(() => toast.classList.add("hidden"), 3500);
}
function setPage(title, eyebrow, action = null) {
  $("#page-title").textContent = eyebrow;
  $("#page-label").textContent = title;
  const pageAction = $("#page-action");
  if (action) {
    pageAction.textContent = action.text;
    pageAction.href = action.href;
    pageAction.classList.remove("hidden");
  } else {
    pageAction.classList.add("hidden");
  }
  const routeSection = (location.hash.split("/")[1] || "tools").split("?")[0];
  const section = routeSection === "register" ? "tools" : routeSection;
  document.querySelectorAll("[data-nav]").forEach((item) => item.classList.toggle("active", item.dataset.nav === section));
}

async function api(path, options = {}) {
  const method = (options.method || "GET").toUpperCase();
  const headers = new Headers(options.headers || {});
  if (options.body && !headers.has("Content-Type")) headers.set("Content-Type", "application/json");
  if (!["GET", "HEAD", "OPTIONS"].includes(method) && state.csrf) headers.set("X-JitForge-Csrf", state.csrf);
  const response = await fetch(path, { ...options, method, headers, credentials: "same-origin" });
  const contentType = response.headers.get("content-type") || "";
  const body = contentType.includes("application/json") ? await response.json() : await response.text();
  if (!response.ok) {
    if (response.status === 401 && path !== "/v1/session") showLogin();
    const error = new Error(body?.message || body || `HTTP ${response.status}`);
    error.code = body?.code;
    error.requestId = body?.request_id;
    throw error;
  }
  return body;
}

function showLogin(message = "") {
  state.csrf = null;
  if (state.pollTimer) window.clearTimeout(state.pollTimer);
  $("#app-shell").classList.add("hidden");
  $("#login-view").classList.remove("hidden");
  $("#login-error").textContent = message;
  $("#login-token").value = "";
  $("#login-token").focus();
}

function showApp(session) {
  state.csrf = session.csrf_token;
  state.expiresAt = session.expires_at;
  $("#login-view").classList.add("hidden");
  $("#app-shell").classList.remove("hidden");
  route();
}

async function restoreSession() {
  try {
    showApp(await api("/v1/session"));
  } catch (_) {
    showLogin();
  }
}

function panel(title, body, action = null) {
  const header = element("div", { className: "panel-header" }, [element("h2", { text: title })]);
  if (action) header.append(action);
  return element("section", { className: "panel" }, [header, element("div", { className: "panel-body" }, body)]);
}

function errorView(error) {
  const detail = error.requestId ? `${error.message}（请求 ${error.requestId}）` : error.message;
  return panel("操作失败", element("p", { className: "error", text: detail }));
}

async function route() {
  if (!state.csrf) return;
  if (state.pollTimer) window.clearTimeout(state.pollTimer);
  if (state.toolsController) state.toolsController.abort();
  const view = $("#view");
  clear(view);
  const hash = location.hash || "#/tools";
  try {
    if (hash === "#/tools") return await renderTools(view);
    if (hash.startsWith("#/register")) return renderRegister(view);
    if (hash === "#/jobs") return await renderJobs(view);
    if (hash === "#/status") return await renderStatus(view);
    if (hash.startsWith("#/jobs/")) return await renderJob(view, decodeURIComponent(hash.slice(7)));
    if (hash.startsWith("#/tools/")) return await renderTool(view, decodeURIComponent(hash.slice(8)));
    location.hash = "#/tools";
  } catch (error) {
    view.append(errorView(error));
  }
}

async function healthProbe(path) {
  try {
    const response = await fetch(path, { cache: "no-store", credentials: "same-origin" });
    return { reachable: true, ok: response.ok, body: await response.json() };
  } catch (error) {
    return { reachable: false, ok: false, body: null, error };
  }
}

function systemStatusRow(name, description, ready, meta) {
  return element("article", { className: `status-row ${ready ? "ready" : "failed"}` }, [
    element("span", { className: "status-dot" }),
    element("div", { className: "status-copy" }, [element("h3", { text: name }), element("p", { text: description })]),
    element("div", { className: "status-value" }, [
      element("strong", { text: ready ? "运行正常" : "当前不可用" }),
      element("span", { text: meta })
    ])
  ]);
}

async function renderStatus(view) {
  setPage("运行状态", "System Status");
  const checkedAt = element("span", { className: "panel-meta", text: "尚未检查" });
  const refresh = element("button", { className: "ghost", text: "重新检查", type: "button" });
  const actions = element("div", { className: "panel-actions" }, [checkedAt, refresh]);
  const content = element("div", {}, loadingRows(3));
  view.append(panel("服务组件", content, actions));

  const load = async () => {
    refresh.disabled = true;
    refresh.textContent = "检查中…";
    const [health, readiness] = await Promise.all([healthProbe("/healthz"), healthProbe("/readyz")]);
    const serverReady = health.reachable && health.ok && health.body?.status === "ok";
    const databaseReady = readiness.reachable && Boolean(readiness.body?.database);
    const workerReady = readiness.reachable && Boolean(readiness.body?.worker);
    const allReady = serverReady && databaseReady && workerReady;
    const version = health.body?.version ? `v${health.body.version}` : "版本未知";

    clear(content);
    content.append(
      element("div", { className: `status-overview ${allReady ? "ready" : "failed"}` }, [
        element("span", { className: "status-dot" }),
        element("div", {}, [
          element("strong", { text: allReady ? "所有必要组件运行正常" : "部分必要组件尚未就绪" }),
          element("p", { text: allReady ? "工具注册、合成与调用链路均可用。" : "请查看下方组件状态定位不可用环节。" })
        ])
      ]),
      element("div", { className: "status-list" }, [
        systemStatusRow("JITForge Server", "HTTP API 与管理界面请求入口", serverReady, version),
        systemStatusRow("Registry / PostgreSQL", "工具、版本与任务的持久化存储", databaseReady, "实时连接检查"),
        systemStatusRow("Worker", "合成任务领取、验证与发布执行器", workerReady, "最近 30 秒心跳")
      ])
    );
    checkedAt.textContent = `检查于 ${new Date().toLocaleTimeString("zh-CN")}`;
    refresh.disabled = false;
    refresh.textContent = "重新检查";
  };

  refresh.addEventListener("click", load);
  await load();
}

async function renderTools(view, offset = 0, search = "", includeUnready = false) {
  setPage("工具", "Capabilities", { text: "注册新工具", href: "#/register" });
  const searchInput = element("input", { type: "search", value: search, placeholder: "搜索名称或能力描述" });
  const include = element("input", { type: "checkbox" });
  include.checked = includeUnready;
  const form = element("form", { className: "toolbar" }, [
    searchInput,
    element("label", { className: "inline-check" }, [include, document.createTextNode("包含未就绪/已撤销")]),
    element("button", { className: "primary compact-button", text: "搜索", type: "submit" })
  ]);
  const resultMeta = element("span", { className: "panel-meta", text: "加载中" });
  const results = element("div", { className: "results-region tool-results" }, loadingCards());
  const toolPanel = panel("能力列表", [form, results], resultMeta);
  const jobsRegion = element("div", { className: "results-region" }, loadingRows(3));
  view.append(toolPanel, panel("最近合成任务", jobsRegion));

  const load = async (nextOffset = 0) => {
    if (state.toolsController) state.toolsController.abort();
    const controller = new AbortController();
    state.toolsController = controller;
    const parameters = new URLSearchParams({ query: searchInput.value.trim(), include_unready: String(include.checked), limit: "50", offset: String(nextOffset) });
    results.classList.add("updating");
    resultMeta.textContent = "正在更新…";
    try {
      const tools = await api(`/v1/tools?${parameters}`, { signal: controller.signal });
      if (state.toolsController !== controller) return;
      resultMeta.textContent = `${tools.tools.length} 个结果`;
      clear(results);
      results.append(renderToolsResult(tools, nextOffset, () => load(Math.max(0, nextOffset - 50)), (value) => load(value)));
    } catch (error) {
      if (error.name !== "AbortError") {
        clear(results); results.append(errorView(error)); resultMeta.textContent = "加载失败";
      }
    } finally {
      if (state.toolsController === controller) {
        results.classList.remove("updating");
        state.toolsController = null;
      }
    }
  };

  form.addEventListener("submit", (event) => { event.preventDefault(); load(0); });
  load(offset);
  try {
    const jobs = await api("/v1/jobs?limit=8&offset=0");
    clear(jobsRegion); jobsRegion.append(jobsTable(jobs.jobs));
  } catch (error) {
    clear(jobsRegion); jobsRegion.append(errorView(error));
  }
}

function renderToolsResult(tools, offset, previousPage, nextPage) {
  const viewport = element("div", { className: "capability-viewport" });
  const grid = element("div", { className: "capability-grid" });
  if (tools.tools.length) {
    tools.tools.forEach((tool, index) => grid.append(capabilityCard(tool, offset + index + 1)));
  } else {
    grid.append(element("div", { className: "capability-empty" }, [
      element("span", { className: "empty-glyph", text: "∅" }),
      element("strong", { text: "没有匹配的能力" }),
      element("span", { text: "调整关键词或状态范围后再次搜索" })
    ]));
  }
  viewport.append(grid);
  const pager = element("div", { className: "form-actions" });
  if (offset > 0) {
    const previous = element("button", { className: "ghost", text: "上一页", type: "button" });
    previous.addEventListener("click", previousPage);
    pager.append(previous);
  }
  if (tools.next_offset !== undefined) {
    const next = element("button", { className: "ghost", text: "下一页", type: "button" });
    next.addEventListener("click", () => nextPage(tools.next_offset));
    pager.append(next);
  }
  return element("div", {}, [viewport, pager]);
}

function capabilityCard(tool, index) {
  const card = element("article", { className: "capability-card" });
  const top = element("div", { className: "capability-card-top" }, [
    element("span", { className: "card-kicker", text: `${String(index).padStart(2, "0")} / capability` }),
    badge(tool.status)
  ]);
  const title = element("h3", { text: tool.tool });
  const description = element("p", { className: "capability-description", text: tool.description });
  const footer = element("div", { className: "capability-footer" }, [
    element("div", { className: "tech-tags" }, [
      element("span", { className: "tech-tag", text: `${tool.input_format} → ${tool.output_format}` }),
      element("span", { className: "tech-tag", text: `stable / ${tool.stable_revision ?? "—"}` })
    ]),
    element("span", { className: "capability-link", text: `rev ${tool.latest_revision}  ↗` })
  ]);
  card.append(top, title, description, footer);
  card.addEventListener("click", () => { location.hash = `#/tools/${encodeURIComponent(tool.tool)}`; });
  card.tabIndex = 0;
  card.addEventListener("keydown", (event) => { if (["Enter", " "].includes(event.key)) { event.preventDefault(); card.click(); } });
  return card;
}

function loadingRows(count = 5) {
  const container = element("div", { className: "loading-list" });
  for (let index = 0; index < count; index += 1) container.append(element("div", { className: "loading-row" }));
  return container;
}

function loadingCards() {
  const container = element("div", { className: "capability-viewport" });
  const grid = element("div", { className: "capability-grid" });
  for (let index = 0; index < 6; index += 1) grid.append(element("div", { className: "loading-card" }));
  container.append(grid);
  return container;
}

function tableHead(labels) {
  const head = element("thead");
  const row = element("tr");
  labels.forEach((label) => row.append(element("th", { text: label })));
  head.append(row);
  return head;
}

function jobsTable(jobs) {
  if (!jobs.length) return element("div", { className: "empty", text: "还没有合成任务" });
  const table = element("table");
  table.append(tableHead(["工具版本", "任务状态", "版本状态", "阶段", "更新时间", "错误"]));
  const body = element("tbody");
  for (const job of jobs) {
    const row = element("tr", { className: "clickable" });
    row.append(
      element("td", { text: `${job.tool}@${job.revision}` }),
      element("td", {}, badge(job.status)),
      element("td", {}, badge(job.version_status)),
      element("td", { text: statusText(job.stage) }),
      element("td", { text: formatTime(job.updated_at) }),
      element("td", { className: "description-column" }, element("div", { className: "description-cell", text: job.error?.message || "—" }))
    );
    row.addEventListener("click", () => { location.hash = `#/jobs/${job.job_id}`; });
    body.append(row);
  }
  table.append(body);
  return element("div", { className: "table-wrap" }, table);
}

async function renderJobs(view, offset = 0, status = "") {
  setPage("合成任务", "Synthesis Jobs");
  const select = element("select");
  [["", "全部状态"], ["queued", "排队中"], ["running", "处理中"], ["ready", "已就绪"], ["rejected", "已拒绝"]].forEach(([value, text]) => select.append(element("option", { value, text })));
  select.value = status;
  select.addEventListener("change", () => { clear(view); renderJobs(view, 0, select.value); });
  const params = new URLSearchParams({ limit: "50", offset: String(offset) });
  if (status) params.set("status", status);
  const response = await api(`/v1/jobs?${params}`);
  const pager = element("div", { className: "form-actions" });
  if (offset > 0) {
    const previous = element("button", { className: "ghost", text: "上一页", type: "button" });
    previous.addEventListener("click", () => { clear(view); renderJobs(view, Math.max(0, offset - 50), status); });
    pager.append(previous);
  }
  if (response.next_offset !== undefined) {
    const next = element("button", { className: "ghost", text: "下一页", type: "button" });
    next.addEventListener("click", () => { clear(view); renderJobs(view, response.next_offset, status); });
    pager.append(next);
  }
  const body = [element("div", { className: "toolbar" }, [select]), jobsTable(response.jobs), pager];
  view.append(panel("最近任务", body));
}

async function renderJob(view, jobId) {
  setPage("任务详情", "Job Detail");
  const job = await api(`/v1/jobs/${encodeURIComponent(jobId)}`);
  const rows = definitionList([
    ["Job ID", job.job_id], ["工具", `${job.tool}@${job.revision}`], ["状态", job.status], ["阶段", job.stage],
    ["版本状态", job.version_status], ["创建时间", formatTime(job.created_at)], ["更新时间", formatTime(job.updated_at)],
    ["错误", job.error ? `${job.error.code}: ${job.error.message}` : "—"]
  ]);
  const action = job.status === "ready" ? element("a", { className: "button primary", text: "查看工具", href: `#/tools/${encodeURIComponent(job.tool)}@${job.revision}` }) : null;
  view.append(panel(`${job.tool}@${job.revision}`, rows, action));
  if (!["ready", "rejected"].includes(job.status)) {
    state.pollTimer = window.setTimeout(() => {
      if (location.hash === `#/jobs/${jobId}`) route();
    }, 1000);
  }
}

function definitionList(entries) {
  const list = element("dl", { className: "definition-list" });
  entries.forEach(([term, value]) => list.append(element("dt", { text: term }), element("dd", { text: value ?? "—" })));
  return list;
}

async function renderTool(view, reference) {
  const at = reference.lastIndexOf("@");
  const name = at > 0 ? reference.slice(0, at) : reference;
  const revision = at > 0 ? Number(reference.slice(at + 1)) : null;
  setPage("工具能力", name);
  const inspectUrl = revision ? `/v1/tools/${encodeURIComponent(name)}?revision=${revision}` : `/v1/tools/${encodeURIComponent(name)}`;
  const [tool, versions] = await Promise.all([api(inspectUrl), api(`/v1/tools/${encodeURIComponent(name)}/versions?limit=100&offset=0`)]);
  const selected = tool.selected;
  const hero = element("div", { className: "detail-hero" }, [
    element("div", {}, [badge(selected.status), element("h2", { text: `${name}@${selected.revision}` }), element("p", { text: selected.description })])
  ]);
  const actions = element("div", { className: "toolbar" });
  const newVersion = element("a", { className: "button primary", text: "注册新版本", href: `#/register?name=${encodeURIComponent(name)}` });
  actions.append(newVersion);
  if (["ready", "deprecated"].includes(selected.status)) {
    const revoke = element("button", { className: "danger", text: "撤销此版本", type: "button" });
    revoke.addEventListener("click", () => revokeVersion(name, selected.revision));
    actions.append(revoke);
  }
  hero.append(actions);
  const info = definitionList([
    ["原始 Intent", selected.requested_intent], ["正式描述", selected.description], ["Stable", tool.stable_revision ?? "无"],
    ["Latest", tool.latest_revision], ["输入输出", `${selected.input_format} → ${selected.output_format}`],
    ["Artifact", selected.artifact_digest || "无"], ["错误", selected.error ? `${selected.error.code}: ${selected.error.message}` : "—"]
  ]);
  const overview = element("div", {}, [hero, info]);
  if (selected.assumptions.length) {
    overview.append(element("h3", { text: "假设" }), element("ul", { className: "tag-list" }, selected.assumptions.map((item) => element("li", { text: item }))));
  }
  view.append(panel("能力概览", overview));
  if (selected.contract) view.append(panel("正式契约", codeBlock(JSON.stringify(selected.contract, null, 2))));
  if (selected.validation_summary) view.append(panel("验证摘要", codeBlock(JSON.stringify(selected.validation_summary, null, 2))));
  view.append(panel("版本历史", versionsTable(name, versions)));
  if (selected.artifact_digest) view.append(panel("源码与测试", artifactViewer(name, selected.revision)));
  if (selected.status === "ready") view.append(panel("试运行", invocationForm(name, selected)));
}

function versionsTable(name, response) {
  const table = element("table");
  table.append(tableHead(["版本", "状态", "Stable", "格式", "更新时间", "描述"]));
  const body = element("tbody");
  for (const version of response.versions) {
    const row = element("tr", { className: "clickable" });
    row.append(
      element("td", { text: version.revision }), element("td", {}, badge(version.status)),
      element("td", { text: response.stable_revision === version.revision ? "是" : "" }),
      element("td", { text: `${version.input_format} → ${version.output_format}` }),
      element("td", { text: formatTime(version.updated_at) }), element("td", { className: "description-column" }, element("div", { className: "description-cell", text: version.description }))
    );
    row.addEventListener("click", () => { location.hash = `#/tools/${encodeURIComponent(name)}@${version.revision}`; });
    body.append(row);
  }
  table.append(body);
  return element("div", { className: "table-wrap" }, table);
}

function artifactViewer(name, revision) {
  const container = element("div");
  const button = element("button", { className: "ghost", text: "加载源码与测试", type: "button" });
  button.addEventListener("click", async () => {
    button.disabled = true;
    button.textContent = "加载中…";
    try {
      const artifact = await api(`/v1/tools/${encodeURIComponent(name)}/versions/${revision}/artifact`);
      clear(container);
      const tabs = element("div", { className: "tabs" });
      const content = element("div");
      const showSource = () => {
        clear(content); content.append(codeBlock(artifact.source));
        [...tabs.children].forEach((item, index) => item.classList.toggle("active", index === 0));
      };
      const showTests = () => {
        clear(content);
        artifact.tests.forEach((test) => content.append(testCard(test)));
        [...tabs.children].forEach((item, index) => item.classList.toggle("active", index === 1));
      };
      const sourceTab = element("button", { text: "源码", type: "button" });
      const testsTab = element("button", { text: `测试 · ${artifact.tests.length}`, type: "button" });
      sourceTab.addEventListener("click", showSource); testsTab.addEventListener("click", showTests);
      tabs.append(sourceTab, testsTab); container.append(tabs, content); showSource();
    } catch (error) { clear(container); container.append(errorView(error)); }
  });
  container.append(button);
  return container;
}

function testCard(test) {
  return element("article", { className: "test-card" }, [
    element("h4", { text: test.name }), element("p", { className: "muted", text: `参数: ${test.args.join(" ") || "无"} · 期望退出码: ${test.expected_exit_code}` }),
    element("strong", { text: "stdin" }), codeBlock(test.stdin), element("strong", { text: "expected stdout" }), codeBlock(test.expected_stdout)
  ]);
}

function invocationForm(name, selected) {
  const form = element("form", { className: "stack" });
  const input = element("textarea", { placeholder: selected.input_format === "json" ? "输入 JSON" : "输入文本" });
  const file = element("input", { type: "file" });
  const args = element("textarea", { className: "short-textarea", placeholder: "每行一个参数" });
  const timeout = element("input", { type: "number", value: "5" }); timeout.min = "1"; timeout.max = "30";
  const output = element("div");
  file.addEventListener("change", async () => { if (file.files[0]) input.value = await readFile(file.files[0], 4 * 1024 * 1024); });
  form.append(
    element("label", {}, [document.createTextNode("标准输入"), input]),
    element("label", {}, [document.createTextNode("从文件读取（最大 4 MiB）"), file]),
    element("label", {}, [document.createTextNode("参数"), args]),
    element("label", {}, [document.createTextNode("超时（秒）"), timeout]),
    element("button", { className: "primary", text: "执行工具", type: "submit" }), output
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault(); clear(output); output.append(element("p", { className: "muted", text: "执行中…" }));
    try {
      const response = await api(`/v1/tools/${encodeURIComponent(name)}/invocations`, {
        method: "POST",
        body: JSON.stringify({ revision: selected.revision, args: args.value.split("\n").filter(Boolean), content_type: selected.input_format === "json" ? "application/json" : "text/plain", stdin_base64: textToBase64(input.value), timeout_ms: Number(timeout.value) * 1000 })
      });
      let stdout = base64ToText(response.stdout_base64); const stderr = base64ToText(response.stderr_base64);
      if (selected.output_format === "json") { try { stdout = JSON.stringify(JSON.parse(stdout), null, 2); } catch (_) {} }
      clear(output); output.append(element("p", { className: "muted", text: `退出码 ${response.exit_code} · ${response.duration_ms} ms · revision ${response.resolved_revision}` }), element("strong", { text: "stdout" }), codeBlock(stdout));
      if (stderr) output.append(element("strong", { text: "stderr" }), codeBlock(stderr));
    } catch (error) { clear(output); output.append(errorView(error)); }
  });
  return form;
}

async function revokeVersion(name, revision) {
  const reason = window.prompt(`请输入撤销 ${name}@${revision} 的原因：`);
  if (!reason?.trim()) return;
  if (!window.confirm(`确认撤销 ${name}@${revision}？默认稳定版本可能发生回退。`)) return;
  try {
    const response = await api(`/v1/tools/${encodeURIComponent(name)}/versions/${revision}/revoke`, { method: "POST", body: JSON.stringify({ reason: reason.trim() }) });
    showToast(response.stable_revision ? `已撤销，stable 回退到 @${response.stable_revision}` : "已撤销，当前没有可调用的稳定版本");
    location.hash = `#/tools/${encodeURIComponent(name)}@${revision}`; await route();
  } catch (error) { showToast(error.message); }
}

function renderRegister(view) {
  const query = new URLSearchParams(location.hash.split("?")[1] || "");
  setPage(query.get("name") ? "注册新版本" : "注册工具", query.get("name") ? "Register Revision" : "Register Tool");
  const form = element("form", { className: "stack" });
  const name = element("input", { value: query.get("name") || "", placeholder: "例如 lscpu-summary", required: true });
  const intent = element("textarea", { placeholder: "描述要生成的确定性、无状态 Unix 工具能力", required: true });
  const inputFormat = formatSelect(); const outputFormat = formatSelect();
  const sample = element("textarea", { placeholder: "可选：粘贴一份代表性输入样本" });
  const sampleFile = element("input", { type: "file" });
  const examples = element("div");
  const addExample = element("button", { className: "ghost", text: "添加输入/输出 Example", type: "button" });
  addExample.addEventListener("click", () => examples.append(exampleRow()));
  sampleFile.addEventListener("change", async () => { if (sampleFile.files[0]) sample.value = await readFile(sampleFile.files[0], 256 * 1024); });
  const formats = element("div", { className: "grid-2" }, [element("label", {}, [document.createTextNode("输入格式"), inputFormat]), element("label", {}, [document.createTextNode("输出格式"), outputFormat])]);
  form.append(
    element("label", {}, [document.createTextNode("工具名称"), name]), element("label", {}, [document.createTextNode("用户 Intent"), intent]), formats,
    element("div", { className: "notice", text: "注意：Input Sample 会进入模型上下文与 Agent Trace。不要提交密钥或敏感生产数据。" }),
    element("label", {}, [document.createTextNode("Input Sample（可选）"), sample]), element("label", {}, [document.createTextNode("从文本文件读取样本"), sampleFile]),
    element("div", { className: "section-heading" }, [element("h3", { text: "严格 Examples" }), addExample]), examples,
    element("div", { className: "form-actions" }, element("button", { className: "primary", text: "开始合成", type: "submit" }))
  );
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const paired = [...examples.querySelectorAll(".example-row")].map((row) => ({ input: row.querySelector("[data-input]").value, output: row.querySelector("[data-output]").value }));
    try {
      const response = await api(`/v1/tools/${encodeURIComponent(name.value.trim())}/registrations`, {
        method: "POST", headers: { "Idempotency-Key": crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}` },
        body: JSON.stringify({ intent: intent.value.trim(), input_format: inputFormat.value, output_format: outputFormat.value, examples: paired, input_samples: sample.value ? [sample.value] : [] })
      });
      showToast(`已创建 ${response.tool}@${response.revision}`); location.hash = `#/jobs/${response.job_id}`;
    } catch (error) { showToast(error.message); }
  });
  view.append(panel("描述它，生成它，调用它", form));
}

function formatSelect() {
  const select = element("select");
  select.append(element("option", { value: "text", text: "text" }), element("option", { value: "json", text: "json" }));
  return select;
}

function exampleRow() {
  const input = element("textarea", { placeholder: "输入" }); input.dataset.input = "true";
  const output = element("textarea", { placeholder: "期望输出" }); output.dataset.output = "true";
  const remove = element("button", { className: "ghost small-button", text: "移除", type: "button" });
  const row = element("div", { className: "example-row" }, [input, output, remove]);
  remove.addEventListener("click", () => row.remove());
  return row;
}

async function readFile(file, limit) {
  if (file.size > limit) throw new Error(`文件超过 ${Math.round(limit / 1024)} KiB 限制`);
  return file.text();
}

function textToBase64(value) {
  const bytes = new TextEncoder().encode(value); let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 0x8000) binary += String.fromCharCode(...bytes.subarray(offset, offset + 0x8000));
  return btoa(binary);
}
function base64ToText(value) {
  const binary = atob(value); const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return new TextDecoder().decode(bytes);
}

$("#login-form").addEventListener("submit", async (event) => {
  event.preventDefault(); $("#login-error").textContent = "";
  const token = $("#login-token").value;
  try {
    const response = await fetch("/v1/session", { method: "POST", headers: { "Content-Type": "application/json" }, credentials: "same-origin", body: JSON.stringify({ token }) });
    const body = await response.json();
    if (!response.ok) throw new Error(body.message || "登录失败");
    showApp(body);
  } catch (error) { showLogin(error.message); }
});

$("#logout-button").addEventListener("click", async () => {
  try { await api("/v1/session", { method: "DELETE" }); } catch (_) {}
  showLogin("已退出登录");
});

window.addEventListener("hashchange", route);
restoreSession();
