---
marp: true
theme: default
paginate: true
html: true
size: 16:9
lang: zh-CN
title: JITForge — 面向 Agent 与 CI 的能力生成与发布平台
description: Contract、Verifier、Artifact、Registry 与受约束执行
style: |
  :root {
    --field: #e3e8ec;
    --paper: #f7f8f8;
    --paper-strong: #ffffff;
    --paper-muted: #edf1f3;
    --ink: #111820;
    --ink-soft: #46515b;
    --muted: #6f7881;
    --rule: #c5cdd3;
    --rule-strong: #98a4ad;
    --blue: #1f56c2;
    --blue-soft: #e7edfa;
    --green: #18724c;
    --green-soft: #e4efe9;
    --amber: #98691b;
    --amber-soft: #f5eee0;
    --red: #a3483c;
    --red-soft: #f4e8e6;
    --code: #151c22;
    --code-text: #edf3f7;
  }
  section {
    box-sizing: border-box;
    border-top: 6px solid var(--ink);
    padding: 42px 54px 38px;
    color: var(--ink);
    background: var(--field);
    font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", "Droid Sans Fallback", Arial, sans-serif;
    font-size: 19px;
    letter-spacing: -.012em;
  }
  section::after {
    right: 30px;
    bottom: 20px;
    color: var(--muted);
    font-family: "Noto Sans Mono", ui-monospace, monospace;
    font-size: 11px;
  }
  h1, h2, h3 { color: var(--ink); }
  h1 { margin: 0; font-size: 49px; line-height: 1.08; letter-spacing: -.052em; }
  h2 { margin: 0 0 22px; font-size: 34px; line-height: 1.12; letter-spacing: -.042em; }
  h3 { margin: 0 0 8px; font-size: 17px; letter-spacing: -.018em; }
  p { line-height: 1.55; }
  strong { color: inherit; }
  code, .mono {
    font-family: "Noto Sans Mono", ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  }
  code { background: rgba(17,24,32,.06); color: var(--ink); }
  pre {
    margin: 0;
    border: 1px solid #303b44;
    border-radius: 2px;
    padding: 14px 17px;
    color: var(--code-text);
    background: var(--code);
    font-size: 13px;
    line-height: 1.5;
  }
  pre code { background: transparent; color: inherit; }
  pre code .hljs-string, pre code .hljs-quote, pre code .hljs-regexp { color: #f2ce83 !important; }
  pre code .hljs-number, pre code .hljs-literal, pre code .hljs-attr { color: #9dc7ff !important; }
  table { display: table; width: 100%; border-collapse: collapse; background: var(--paper-strong); font-size: 14px; }
  th {
    color: var(--muted);
    background: var(--paper-muted);
    font-family: "Noto Sans Mono", ui-monospace, monospace;
    font-size: 10px;
    letter-spacing: .055em;
    text-align: left;
    text-transform: uppercase;
  }
  th, td { border-bottom: 1px solid var(--rule); padding: 8px 10px; vertical-align: top; }
  .value-table { table-layout: fixed; }
  .value-table th:first-child, .value-table td:first-child { width: 170px; }
  .value-table th:last-child, .value-table td:last-child { width: 320px; }
  ul, ol { line-height: 1.5; }
  li::marker { color: var(--blue); }
  .docline {
    margin: 0 0 12px;
    color: var(--blue);
    font-family: "Noto Sans Mono", ui-monospace, monospace;
    font-size: 11px;
    font-weight: 800;
    letter-spacing: .08em;
    text-transform: uppercase;
  }
  .lead { color: var(--ink-soft); font-size: 22px; line-height: 1.52; }
  .small { font-size: 15px; line-height: 1.55; }
  .tiny { font-size: 12px; line-height: 1.5; }
  .muted { color: var(--muted); }
  .blue { color: var(--blue); }
  .green { color: var(--green); }
  .amber { color: var(--amber); }
  .red { color: var(--red); }
  .sheet { border-top: 1px solid var(--rule-strong); border-bottom: 1px solid var(--rule-strong); background: var(--paper); }
  .sheet-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    min-height: 38px;
    padding: 0 12px;
    border-bottom: 1px solid var(--rule);
    color: var(--muted);
    background: var(--paper-muted);
    font-family: "Noto Sans Mono", ui-monospace, monospace;
    font-size: 10px;
    letter-spacing: .055em;
    text-transform: uppercase;
  }
  .terminal { border-left: 4px solid var(--blue); }
  .terminal .prompt { color: #99bfff; }
  .terminal .ok { color: #8ad0aa; }
  .terminal .warn { color: #f0cc82; }
  .terminal .dim { color: #8f9aa3; }
  .record-rows { margin: 0; }
  .record-row {
    min-height: 40px;
    display: grid;
    grid-template-columns: 110px minmax(0, 1fr) auto;
    align-items: center;
    gap: 14px;
    padding: 7px 12px;
    border-bottom: 1px solid var(--rule);
  }
  .record-row:last-child { border-bottom: 0; }
  .record-key { color: var(--muted); font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 10px; letter-spacing: .04em; text-transform: uppercase; }
  .record-value { min-width: 0; font-size: 14px; overflow-wrap: anywhere; }
  .stamp {
    width: max-content;
    border: 1px solid currentColor;
    border-radius: 2px;
    padding: 3px 6px;
    font-family: "Noto Sans Mono", ui-monospace, monospace;
    font-size: 10px;
    font-weight: 800;
    letter-spacing: .04em;
    text-transform: uppercase;
  }
  .stamp.ready { color: var(--green); background: var(--green-soft); }
  .stamp.wait { color: var(--amber); background: var(--amber-soft); }
  .stamp.stop { color: var(--red); background: var(--red-soft); }
  .spine { border-left: 4px solid var(--blue); }
  .spine-row {
    min-height: 54px;
    display: grid;
    grid-template-columns: 42px 145px minmax(0, 1fr);
    align-items: center;
    gap: 10px;
    padding: 7px 14px;
    border-bottom: 1px solid var(--rule);
    background: var(--paper-strong);
  }
  .spine-row:last-child { border-bottom: 0; }
  .spine-index { color: var(--muted); font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 10px; }
  .spine-object { font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 12px; font-weight: 800; text-transform: uppercase; }
  .spine-detail { color: var(--ink-soft); font-size: 14px; }
  .cols { display: grid; gap: 18px; }
  .cols.two { grid-template-columns: repeat(2, minmax(0, 1fr)); }
  .cols.uneven { grid-template-columns: minmax(0, 1.15fr) minmax(0, .85fr); }
  .callout { border-left: 4px solid var(--blue); padding: 10px 14px; background: var(--paper-strong); font-size: 14px; line-height: 1.55; }
  .inline-facts { display: flex; flex-wrap: wrap; gap: 0; border-top: 1px solid var(--rule-strong); border-bottom: 1px solid var(--rule-strong); background: var(--paper); }
  .inline-facts > div { flex: 1 1 130px; padding: 10px 12px; border-right: 1px solid var(--rule); }
  .inline-facts > div:last-child { border-right: 0; }
  .fact-value { font-size: 22px; font-weight: 780; letter-spacing: -.035em; }
  .fact-label { margin-top: 2px; color: var(--muted); font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 9px; letter-spacing: .04em; text-transform: uppercase; }
  section[data-class~="cover"] { border-top-color: #8fb2ff; color: #eef3f7; background: var(--ink); }
  section[data-class~="cover"] h1, section[data-class~="cover"] h2, section[data-class~="cover"] h3 { color: #fff; }
  section[data-class~="cover"] .docline { color: #8fb2ff; }
  section[data-class~="cover"] .lead { color: #c7d0d8; }
  section[data-class~="cover"]::after { color: #87939d; }
  .cover-layout { height: 100%; display: grid; grid-template-columns: minmax(0, 1.2fr) minmax(360px, .8fr); align-items: center; gap: 55px; }
  .cover h1 { max-width: 760px; font-size: 52px; }
  .cover-receipt { border-top: 1px solid #61707c; border-bottom: 1px solid #61707c; background: #1a232b; }
  .cover-receipt .sheet-head { border-color: #46535e; color: #94a0aa; background: #202b34; }
  .cover-receipt .record-row { border-color: #3f4b55; }
  .cover-receipt .record-key { color: #87939d; }
  .cover-receipt .record-value { color: #e7edf2; }
  .cover-command { margin-top: 22px; color: #9dc0ff; font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 14px; }
  .terminal-wide { margin-bottom: 18px; }
  .publish-grid { display: grid; grid-template-columns: minmax(0, 1fr) 310px; gap: 18px; }
  .publish-result { display: grid; align-content: center; padding: 18px; border-left: 4px solid var(--green); background: var(--paper-strong); }
  .publish-result .mono { font-size: 17px; }
  .boundary-command { margin-bottom: 16px; }
  .boundary-table td:first-child { width: 150px; font-family: "Noto Sans Mono", ui-monospace, monospace; font-weight: 800; }
  .boundary-note { margin-top: 14px; }
  .manifest { border-left: 4px solid var(--blue); }
  .manifest .record-row { grid-template-columns: 115px minmax(0, 1fr); }
  .arch-map {
    display: grid;
    grid-template-columns: 170px 44px 210px 44px 210px 44px 220px;
    align-items: center;
    justify-content: center;
    margin: 10px 0 18px;
  }
  .node { min-height: 88px; display: grid; align-content: center; padding: 12px; border-top: 1px solid var(--rule-strong); border-bottom: 1px solid var(--rule-strong); background: var(--paper-strong); }
  .node strong { display: block; font-size: 16px; }
  .node span { color: var(--muted); font-size: 12px; line-height: 1.4; }
  .node.worker { border-left: 4px solid var(--blue); }
  .arrow { color: var(--blue); font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 12px; line-height: 1.35; text-align: center; white-space: nowrap; }
  .state-strip { display: grid; grid-template-columns: repeat(7, minmax(0, 1fr)); border-top: 1px solid var(--rule-strong); border-bottom: 1px solid var(--rule-strong); }
  .state-strip div { min-height: 54px; display: grid; place-content: center; border-right: 1px solid var(--rule); background: var(--paper); font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 10px; text-align: center; }
  .state-strip div:last-child { border-right: 0; }
  .state-strip .active { color: var(--green); background: var(--green-soft); }
  .state-strip .pause { color: var(--amber); background: var(--amber-soft); }
  .evidence-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 0; border-top: 1px solid var(--rule-strong); border-bottom: 1px solid var(--rule-strong); background: var(--paper); }
  .evidence { min-height: 260px; padding: 18px; border-right: 1px solid var(--rule); }
  .evidence:last-child { border-right: 0; }
  .evidence-no { color: var(--blue); font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 11px; }
  .evidence h3 { margin-top: 14px; font-size: 19px; }
  .evidence p { color: var(--ink-soft); font-size: 14px; }
  .tool-surface { margin-top: 14px; padding: 10px 12px; border-left: 4px solid var(--blue); color: var(--ink-soft); background: var(--paper-strong); font-size: 13px; line-height: 1.6; }
  .artifact-stack { border-left: 4px solid var(--blue); }
  .artifact-layer { min-height: 44px; display: grid; grid-template-columns: 130px 1fr; align-items: center; gap: 14px; padding: 7px 12px; border-bottom: 1px solid var(--rule); background: var(--paper-strong); }
  .artifact-layer:last-child { border-bottom: 0; }
  .artifact-layer strong { font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 11px; text-transform: uppercase; }
  .artifact-layer span { color: var(--ink-soft); font-size: 13px; }
  .digest-line { padding: 12px; color: #dbe7f0; background: var(--code); font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 12px; }
  .version-line { margin-top: 16px; }
  .route-map { display: grid; grid-template-columns: minmax(0, 1fr) 56px minmax(0, 1fr) 56px minmax(0, 1fr); align-items: stretch; margin-bottom: 16px; }
  .route-map .arrow { display: grid; place-content: center; font-size: 24px; font-weight: 800; }
  .route-box { display: grid; align-content: center; min-height: 108px; padding: 13px; border-top: 1px solid var(--rule-strong); border-bottom: 1px solid var(--rule-strong); background: var(--paper-strong); }
  .route-box strong { font-size: 16px; }
  .route-box span { color: var(--muted); font-size: 12px; }
  .route-box.edge { border-left: 4px solid var(--blue); }
  .case-layout { display: grid; grid-template-columns: minmax(0, 1.08fr) minmax(0, .92fr); gap: 18px; }
  .test-ledger { border-top: 1px solid var(--rule-strong); border-bottom: 1px solid var(--rule-strong); background: var(--paper-strong); }
  .test-row { min-height: 43px; display: grid; grid-template-columns: 94px minmax(0, 1fr); align-items: center; gap: 12px; padding: 6px 11px; border-bottom: 1px solid var(--rule); }
  .test-row:last-child { border-bottom: 0; }
  .test-row strong { color: var(--green); font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 10px; }
  .test-row span { font-size: 13px; }
  .weather-output { margin-bottom: 14px; }
  .grant-ledger { border-left: 4px solid var(--blue); }
  .grant-row { min-height: 64px; padding: 9px 12px; border-bottom: 1px solid var(--rule); background: var(--paper-strong); }
  .grant-row:last-child { border-bottom: 0; }
  .grant-row code { display: block; margin-bottom: 4px; font-size: 11px; }
  .grant-row span { color: var(--muted); font-size: 12px; }
  .snapshot-ledger { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); border-top: 1px solid var(--rule-strong); border-bottom: 1px solid var(--rule-strong); background: var(--paper); }
  .snapshot-item { min-height: 106px; padding: 16px; border-right: 1px solid var(--rule); border-bottom: 1px solid var(--rule); }
  .snapshot-item:nth-child(3n) { border-right: 0; }
  .snapshot-item:nth-child(n+4) { border-bottom: 0; }
  .snapshot-value { font-size: 38px; font-weight: 780; line-height: 1; letter-spacing: -.05em; }
  .snapshot-label { margin-top: 8px; color: var(--muted); font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 10px; letter-spacing: .04em; text-transform: uppercase; }
  section[data-class~="appendix"] { border-top-color: var(--blue); }
  .defense { display: grid; gap: 0; border-top: 1px solid var(--rule-strong); border-bottom: 1px solid var(--rule-strong); }
  .defense-row { min-height: 61px; display: grid; grid-template-columns: 150px 220px minmax(0, 1fr); align-items: center; gap: 14px; padding: 8px 12px; border-bottom: 1px solid var(--rule); background: var(--paper-strong); }
  .defense-row:last-child { border-bottom: 0; }
  .defense-row strong { font-family: "Noto Sans Mono", ui-monospace, monospace; font-size: 11px; text-transform: uppercase; }
  .defense-row span { color: var(--ink-soft); font-size: 13px; }
---

<!-- _class: cover -->

<div class="cover-layout">
  <div>
    <p class="docline">JITFORGE / RELEASE RECORD 0001</p>
    <h1>面向 Agent 与 CI 的<br>能力生成与发布平台</h1>
    <p class="lead">模型负责生成，JITForge 负责验证、版本管理和后续调用；<br>一次性脚本由此成为可复现的 Unix 能力。</p>
    <p class="cover-command">需求 + 输入样本 + 输出示例 → name@revision</p>
  </div>

  <div class="cover-receipt">
    <div class="sheet-head"><span>EXAMPLE</span><span>PUBLISHED</span></div>
    <div class="record-row"><span class="record-key">NAME</span><span class="record-value mono">git-change-report@1</span></div>
    <div class="record-row"><span class="record-key">FUNCTION</span><span class="record-value">git diff 文件与增删行汇总</span></div>
    <div class="record-row"><span class="record-key">I/O</span><span class="record-value mono">text → json</span></div>
    <div class="record-row"><span class="record-key">TESTS</span><span class="record-value mono">13 / 13</span><span class="stamp ready">pass</span></div>
    <div class="record-row"><span class="record-key">CALL</span><span class="record-value">能力确定后无 AI 参与</span></div>
  </div>
</div>

---

<p class="docline">WHY / HOW</p>

## 问题与方法

<div class="cols two">
  <div class="sheet spine">
    <div class="sheet-head"><span>SHARING GAPS</span><span>BETWEEN AGENTS</span></div>
    <div class="spine-row"><span class="spine-index">01</span><span class="spine-object">SHARING</span><span class="spine-detail">多个 Agent、多个环境之间，能力无法自动共享</span></div>
    <div class="spine-row"><span class="spine-index">02</span><span class="spine-object">SYNC</span><span class="spine-detail">手动同步配置繁琐，也容易出错</span></div>
    <div class="spine-row"><span class="spine-index">03</span><span class="spine-object">REPEAT</span><span class="spine-detail">同一需求在不同 Agent 中往往得到不同实现</span></div>
  </div>

  <div class="sheet manifest">
    <div class="sheet-head"><span>JITFORGE SCOPE</span><span>POST-GENERATION</span></div>
    <div class="record-row"><span class="record-key">MODEL</span><span class="record-value">根据需求生成候选实现</span></div>
    <div class="record-row"><span class="record-key">JITFORGE</span><span class="record-value">检查输入输出、测试和实际运行结果</span></div>
    <div class="record-row"><span class="record-key">VERSION</span><span class="record-value">通过后保存为 <span class="mono">name@revision</span></span></div>
    <div class="record-row"><span class="record-key">CALL</span><span class="record-value">后续直接执行，AI 不再参与</span></div>
  </div>
</div>

<div class="callout" style="margin-top:16px">生成本身有一定难度，JITForge 承担的是其后半段：<strong>验证、版本管理与后续调用</strong>，让一次定义的能力可以共享、复用。</div>

---

<p class="docline">PRODUCT FIT / UX</p>

## 产品定位与交互

<div class="terminal terminal-wide">

```bash
docker ps -a | jit register docker-ps-json \
  "把原始文本转成只含 id、image、status 的 JSON 数组"
```

</div>

<div class="publish-grid">
  <div class="sheet spine">
    <div class="sheet-head"><span>PUBLICATION TRACE</span><span>ONE REQUEST</span></div>
    <div class="spine-row"><span class="spine-index">01</span><span class="spine-object">Input Sample</span><span class="spine-detail">管道 stdin 原样进入注册请求，多行和制表符不用重新转义</span></div>
    <div class="spine-row"><span class="spine-index">02</span><span class="spine-object">Contract</span><span class="spine-detail">先确认要做什么、输入输出和出错方式，再写源码</span></div>
    <div class="spine-row"><span class="spine-index">03</span><span class="spine-object">Validation</span><span class="spine-detail">生成结果经过独立检查，并在受限环境里实际运行</span></div>
    <div class="spine-row"><span class="spine-index">04</span><span class="spine-object">Revision</span><span class="spine-detail">通过后保存版本，得到 <span class="mono">name@revision</span></span></div>
  </div>
  <div class="publish-result">
    <span class="stamp ready">product fit</span>
    <p class="mono">text / json · short-lived · stateless</p>
    <p class="small">适合输入输出明确，并且会在 Agent / CI 中反复出现的胶水任务。</p>
    <p class="small muted">需要版本管理、撤销、回滚或调用记录时，也适合进入 Registry。</p>
  </div>
</div>

---

<p class="docline">POSITIONING</p>

## MCP、Skills 与 JITForge

<table class="boundary-table">
  <thead><tr><th>对象</th><th>主要职责</th><th>生成后的责任主体</th><th>版本 / 撤销</th></tr></thead>
  <tbody>
    <tr><td>Skills</td><td>给 Agent 指令、知识和工作流</td><td>Agent 直接加载并解释</td><td>通常随文件修改</td></tr>
    <tr><td>MCP</td><td>把已有在线服务暴露为工具</td><td>MCP Server 自行实现与运行</td><td>各 Server 自行维护</td></tr>
    <tr><td class="blue">JITForge</td><td>生成并发布确定性的 Unix 能力</td><td>契约审查 + 独立验证 + 受限执行</td><td><span class="mono">revision / stable / revoke</span></td></tr>
  </tbody>
</table>

<div class="callout boundary-note">三者都为 Agent 提供可调用能力。差异出现在生成之后：Skills 直接加载，MCP 连接已有服务；JITForge 还要经过契约审查、独立验证和沙箱执行，最后发布为不可变 revision。</div>

---

<p class="docline">INPUT / PUBLICATION</p>

## 输入模型与发布流程

<div class="cols uneven">
  <div class="sheet manifest">
    <div class="sheet-head"><span>REGISTRATION MANIFEST</span><span>INPUT MODEL</span></div>
    <div class="record-row"><span class="record-key">INTENT</span><span class="record-value">目标问题与操作语义</span></div>
    <div class="record-row"><span class="record-key">INPUT SAMPLE</span><span class="record-value">暴露真实输入形状；只证明格式，不证明正确输出</span></div>
    <div class="record-row"><span class="record-key">STRICT EXAMPLE</span><span class="record-value"><span class="mono">INPUT ⇒ OUTPUT</span>，用户断言，Agent 不能静默改写</span></div>
    <div class="record-row"><span class="record-key">I/O FORMAT</span><span class="record-value"><span class="mono">text / json</span> 协议与 schema-level 约束</span></div>
  </div>

  <div class="sheet spine">
    <div class="sheet-head"><span>PUBLICATION SPINE</span><span>ATOMIC</span></div>
    <div class="spine-row"><span class="spine-index">01</span><span class="spine-object">Request</span><span class="spine-detail">Idempotency-Key</span></div>
    <div class="spine-row"><span class="spine-index">02</span><span class="spine-object">Job</span><span class="spine-detail">PostgreSQL lease</span></div>
    <div class="spine-row"><span class="spine-index">03</span><span class="spine-object">Contract</span><span class="spine-detail">accepted before code</span></div>
    <div class="spine-row"><span class="spine-index">04</span><span class="spine-object">Source</span><span class="spine-detail">controlled edits</span></div>
    <div class="spine-row"><span class="spine-index">05</span><span class="spine-object">Validation</span><span class="spine-detail">runsc execution</span></div>
    <div class="spine-row"><span class="spine-index">06</span><span class="spine-object">Revision</span><span class="spine-detail">artifact + stable pointer</span></div>
  </div>
</div>

<p class="small muted">需要修正 Example 时，Agent 只能提出明确替换；用户批准后继续，原始断言与修正记录都留在 Trace。</p>

---

<p class="docline">STATE / ARCHITECTURE</p>

## 状态机与系统架构

<div class="arch-map">
  <div class="node"><strong>CLI / Web</strong><span>register · inspect · answer · cancel · call</span></div>
  <div class="arrow">HTTP<br>→</div>
  <div class="node"><strong>Nginx + Server</strong><span>Session / CSRF · API · Registry control plane</span></div>
  <div class="arrow">SQL<br>↔</div>
  <div class="node"><strong>PostgreSQL</strong><span>Revision · Job · Contract · Trace · Approval</span></div>
  <div class="arrow">gRPC<br>↔</div>
  <div class="node worker"><strong>Worker</strong><span>Agent · Verifier · Build · runsc · Artifact Store</span></div>
</div>

<div class="state-strip">
  <div>queued</div><div>running</div><div>contract_ready</div><div>synthesizing</div><div>building</div><div>validating</div><div class="active">ready</div>
</div>
<div class="state-strip" style="margin-top:8px;grid-template-columns:repeat(4,1fr)">
  <div class="pause">awaiting_input<br><span class="tiny">clarify / correction / approval</span></div>
  <div>resume from checkpoint</div><div class="stop">cancelled</div><div class="stop">revoked</div>
</div>

<div class="cols two" style="margin-top:16px">
  <div class="callout"><strong>Registry</strong><br>持久化 Contract、Assumptions、Validation 与 Agent Trace；普通调用的 stdin / stdout 正文默认不入库。</div>
  <div class="callout"><strong>Worker</strong><br>模型密钥与 Docker Socket 仅由 Worker 持有；Nginx 与 Server 不接触二者。</div>
</div>

---

<p class="docline">AGENT / CONSTRAINTS</p>

## 合成 Agent 的工作边界

<div class="cols two">
  <div class="sheet spine">
    <div class="sheet-head"><span>ALLOWED WORK</span><span>BOUNDED</span></div>
    <div class="spine-row"><span class="spine-index">01</span><span class="spine-object">TASK</span><span class="spine-detail">单个、短时、无状态的 Unix filter；一份 Python 3 标准库源码</span></div>
    <div class="spine-row"><span class="spine-index">02</span><span class="spine-object">ACTION</span><span class="spine-detail">每轮只允许一个受控 tool call，不接受自由文本结果</span></div>
    <div class="spine-row"><span class="spine-index">03</span><span class="spine-object">HOST</span><span class="spine-detail">无宿主 Bash、持久文件、subprocess 或第三方包</span></div>
    <div class="spine-row"><span class="spine-index">04</span><span class="spine-object">NETWORK</span><span class="spine-detail">默认断网；公开 HTTPS GET 需要先申请精确 Grant</span></div>
  </div>

  <div class="sheet manifest">
    <div class="sheet-head"><span>HARD BUDGET</span><span>PER RUN</span></div>
    <div class="record-row"><span class="record-key">TURNS</span><span class="record-value mono">24 model turns</span></div>
    <div class="record-row"><span class="record-key">SOURCE</span><span class="record-value mono">4 revisions</span></div>
    <div class="record-row"><span class="record-key">TEST REVIEW</span><span class="record-value mono">3 corrections</span></div>
    <div class="record-row"><span class="record-key">PROBE</span><span class="record-value mono">3 agent probes</span></div>
  </div>
</div>

<div class="callout" style="margin-top:16px">Contract 通过独立 review 后才能写源码。User Example 不能静默改写；确实需要修正时，任务暂停并等待用户批准。</div>

---

<p class="docline">QUALITY GATE / READY</p>

## 发布标准

<div class="evidence-grid">
  <div class="evidence">
    <span class="evidence-no">EVIDENCE / 01</span>
    <h3>User Evidence</h3>
    <p>Strict Example 是不可变断言；Input Sample 只提供真实输入形状，不充当正确答案。</p>
    <span class="stamp ready">user supplied</span>
  </div>
  <div class="evidence">
    <span class="evidence-no">EVIDENCE / 02</span>
    <h3>Contract Review</h3>
    <p>独立调用只返回 <span class="mono">accept / revise / reject</span>；它检查语义偷换、错误 Oracle 和样本硬编码。</p>
    <span class="stamp ready">separate review</span>
  </div>
  <div class="evidence">
    <span class="evidence-no">EVIDENCE / 03</span>
    <h3>Sandbox Run</h3>
    <p>候选必须实际 build / run；所有用户测试和生成测试通过，输出格式与 exit code 也要符合 Contract。</p>
    <span class="stamp ready">executed</span>
  </div>
</div>

<div class="callout" style="margin-top:14px">Contract Review = accept，Input Sample 运行符合声明的 I/O，所有用户测试和生成测试通过，才发布 ready revision。Verifier 不能绕过用户断言或 Sandbox 结果。</div>

---

<p class="docline">RUNNER / VERSIONING</p>

## 运行时与版本管理

<div class="cols uneven">
  <div>
    <div class="artifact-stack">
      <div class="artifact-layer"><strong>Manifest</strong><span>runtime、entrypoint、I/O、limits、Capability Grants</span></div>
      <div class="artifact-layer"><strong>Contract</strong><span>summary、assumptions、invariants、error semantics</span></div>
      <div class="artifact-layer"><strong>Source</strong><span>经受控编辑形成的最终源码</span></div>
      <div class="artifact-layer"><strong>Tests</strong><span>用户样例、生成测试与黑盒变体</span></div>
      <div class="artifact-layer"><strong>Validation</strong><span>测试结果、Verifier 结论与 Sandbox evidence</span></div>
      <div class="digest-line">SHA-256(manifest + contract + source + tests + evidence)</div>
    </div>
  </div>
  <div>
    <table>
      <thead><tr><th>runsc constraint</th><th>value</th></tr></thead>
      <tbody>
        <tr><td>User / FS</td><td class="mono">65532 · read-only rootfs · /tmp noexec</td></tr>
        <tr><td>Privilege</td><td class="mono">cap-drop=ALL · no-new-privileges</td></tr>
        <tr><td>Resources</td><td class="mono">128 MiB · 0.5 CPU · nproc 16</td></tr>
        <tr><td>Network</td><td class="mono">build=none · offline run=none</td></tr>
        <tr><td>Output</td><td class="mono">stdout/stderr 1 MiB · hard timeout</td></tr>
      </tbody>
    </table>
  </div>
</div>

<div class="inline-facts version-line">
  <div><div class="fact-value mono">latest</div><div class="fact-label">最后注册的 revision</div></div>
  <div><div class="fact-value mono">stable</div><div class="fact-label">默认可调用 revision</div></div>
  <div><div class="fact-value mono">selected</div><div class="fact-label">本次解析结果</div></div>
  <div><div class="fact-value mono">revoke</div><div class="fact-label">停止调用并回退指针</div></div>
</div>

---

<p class="docline">INTERFACES / CONTROL PLANE</p>

## 控制面与接口

<div class="route-map">
  <div class="route-box edge"><strong>Internet / HTTPS</strong><span>Cloudflare Edge + Tunnel；TLS 在边缘终止</span></div>
  <div class="arrow">→</div>
  <div class="route-box"><strong>Nginx :8080</strong><span>Docker 网络内保持 HTTP，负责路由</span></div>
  <div class="arrow">→</div>
  <div class="route-box"><strong>JITForge Server</strong><span>Landing · Web Console · HTTP API</span></div>
</div>

<table>
  <thead><tr><th>入口</th><th>认证</th><th>用途</th></tr></thead>
  <tbody>
    <tr><td class="mono">jit CLI</td><td>Bearer</td><td>Agent、Shell 与 CI；保留 stdin / stdout / stderr / exit code</td></tr>
    <tr><td>Web Console</td><td>Session + CSRF</td><td>人工注册、审查、批准与撤销</td></tr>
    <tr><td class="mono">HTTP API</td><td>Bearer</td><td>自动化接入；注册请求支持 <span class="mono">Idempotency-Key</span></td></tr>
  </tbody>
</table>

---

<p class="docline">CASE RECORD / OFFLINE FILTER</p>

## `git-change-report@1`

<div class="case-layout">
  <div>

```bash
git diff --numstat --no-renames HEAD~1 HEAD \
  | jit call git-change-report@1 | jq .
```

```json
{"files_changed":1,"insertions":20,"deletions":4,
 "binary_files":0,
 "by_area":[{"area":"apps/jit-worker","files":1,
             "insertions":20,"deletions":4}]}
```

  </div>
  <div class="test-ledger">
    <div class="sheet-head"><span>BLACK-BOX VALIDATION</span><span>13 / 13</span></div>
    <div class="test-row"><strong>PASS</strong><span>行顺序反转，汇总结果保持不变</span></div>
    <div class="test-row"><strong>PASS</strong><span>CRLF、空行、无末尾换行</span></div>
    <div class="test-row"><strong>PASS</strong><span>中文与空格路径原样保留</span></div>
    <div class="test-row"><strong>PASS</strong><span>binary diff 不计入增删行</span></div>
    <div class="test-row"><strong>EXIT 1</strong><span>重复路径：stdout 0 bytes，stderr 给出行号</span></div>
  </div>
</div>

<div class="inline-facts" style="margin-top:15px">
  <div><div class="fact-value">13 / 13</div><div class="fact-label">Artifact tests</div></div>
  <div><div class="fact-value">2</div><div class="fact-label">Agent turns</div></div>
  <div><div class="fact-value">0</div><div class="fact-label">Repair rounds</div></div>
  <div><div class="fact-value mono">6fc89a…</div><div class="fact-label">Content digest</div></div>
</div>

---

<p class="docline">CASE RECORD / HTTP CAPABILITY</p>

## `current-weather@2` / HTTP Capability

<div class="cols two">
  <div>

```bash
printf '上海' | jit call current-weather@2 | jq .
```

<div class="weather-output">

```json
{"place":"上海","local_date":"2026-07-17","weather":"Overcast",
 "temperature":38.6,"feels_like":43.5,
 "temp_max":38.7,"temp_min":31.3,"source":"Open-Meteo"}
```

</div>

<div class="inline-facts">
  <div><div class="fact-value">8 / 8</div><div class="fact-label">tests</div></div>
  <div><div class="fact-value">11</div><div class="fact-label">turns</div></div>
  <div><div class="fact-value">0</div><div class="fact-label">repairs</div></div>
</div>
  </div>
  <div>
    <div class="grant-ledger">
      <div class="sheet-head"><span>GRANTS IN ARTIFACT</span><span>HTTPS GET</span></div>
      <div class="grant-row"><code>geocoding-api.open-meteo.com/v1/search</code><span>Host、Path Prefix、Query Keys 固化在 Grant</span></div>
      <div class="grant-row"><code>api.open-meteo.com/v1/forecast</code><span>每次调用前确认 Approval 仍然有效</span></div>
    </div>

```text
search_web → fetch_document
       ↓
request → awaiting_input → approve → resume
       ↓
probe_http → Grant 写入 Artifact
       ↓
Fixture 合成测试 / 实时调用复用同一 Contract
```

  </div>
</div>

---

<p class="docline">REGISTRY / DEPLOYED SNAPSHOT</p>

## Registry 快照与实际落点

<div class="snapshot-ledger">
  <div class="snapshot-item"><div class="snapshot-value">48</div><div class="snapshot-label">capability names</div></div>
  <div class="snapshot-item"><div class="snapshot-value">69</div><div class="snapshot-label">revisions</div></div>
  <div class="snapshot-item"><div class="snapshot-value">30</div><div class="snapshot-label">stored artifacts</div></div>
  <div class="snapshot-item"><div class="snapshot-value">85</div><div class="snapshot-label">recorded invocations</div></div>
  <div class="snapshot-item"><div class="snapshot-value">59</div><div class="snapshot-label">workspace tests passed</div></div>
  <div class="snapshot-item"><div class="snapshot-value">5</div><div class="snapshot-label">capability approvals</div></div>
</div>

<table class="value-table" style="margin-top:17px">
  <thead><tr><th>使用位置</th><th>已经跑通的链路</th><th>留下的工程记录</th></tr></thead>
  <tbody>
    <tr><td>Agent / Shell / CI</td><td>CLI 注册、等待与调用；按 <span class="mono">name@revision</span> 选择版本</td><td class="mono">Contract · Artifact · Digest</td></tr>
    <tr><td>Web Console</td><td>注册、审查、调用与撤销；任务和系统状态可直接查看</td><td class="mono">Revision · Trace · Invocation</td></tr>
    <tr><td>HTTP Capability</td><td>审批联网范围，Grant 随 Artifact 发布并可撤销</td><td class="mono">Approval · Grant · Audit</td></tr>
  </tbody>
</table>

<p class="small" style="margin-top:15px"><span class="mono">DEPLOYED PATH</span>　Landing / Console / CLI download → Cloudflare Tunnel → Nginx → Server → Registry / Worker</p>

<p class="tiny muted">快照采集于 2026-07-17；数字来自当前 PostgreSQL Registry 与实际 workspace test 列表。</p>

---

<!-- _class: appendix -->

<p class="docline">APPENDIX / DEFENSE IN DEPTH</p>

## 分层安全机制

<div class="defense">
  <div class="defense-row"><strong>Edge / Gateway</strong><span>Cloudflare TLS · Tunnel Origin · Nginx routing</span><span>公网 HTTPS 在边缘终止；内部 HTTP 不伪装成端到端 TLS。</span></div>
  <div class="defense-row"><strong>Control Plane</strong><span>Session · SameSite=Strict · CSRF</span><span>Cookie 证明 Session；浏览器写请求还要匹配 <span class="mono">X-JitForge-Csrf</span>。</span></div>
  <div class="defense-row"><strong>Service Boundary</strong><span>Server ↔ private gRPC ↔ Worker</span><span>模型密钥与 Docker Socket 仅由 Worker 持有。</span></div>
  <div class="defense-row"><strong>Artifact</strong><span>Contract · Source · Tests · Evidence · Grant</span><span>整体进入内容摘要；ready 后不可原地修改。</span></div>
  <div class="defense-row"><strong>Runner</strong><span>runsc · non-root · read-only · limits</span><span>默认断网、最小权限、资源和输出均设硬上限。</span></div>
  <div class="defense-row"><strong>HTTP Data</strong><span>HTTPS GET · public DNS · redirect review</span><span>Host、Path、Query、Approval 与撤销共同约束访问范围。</span></div>
</div>

<p class="small muted">模型不进入调用路径，Artifact 发布后不可原地修改；Runner 默认断网，并且只获得完成调用所需的最小权限。</p>

---

<!-- _class: appendix -->

<p class="docline">ACKNOWLEDGMENTS / PROJECT CREDITS</p>

## ACKNOWLEDGMENTS

<div class="sheet">
  <div class="sheet-head"><span>CREDIT LEDGER</span><span>JITFORGE / 2026-07-17</span></div>
  <div class="record-row"><span class="record-key">INSPIRATION</span><span class="record-value">本项目最初受 <strong>vibeOS</strong> 演示启发</span></div>
  <div class="record-row"><span class="record-key">RIG-CORE</span><span class="record-value">合成 Agent 构建于 <strong>rig-core AgentRun</strong>；rig 提供原生 tool calling、多轮，以及可序列化、可中断恢复的 agent state</span></div>
  <div class="record-row"><span class="record-key">JITFORGE</span><span class="record-value">JITForge 在其上实现有界 synthesis loop，并自主实现 Contract、独立 Verifier、gVisor sandbox、content-addressed Registry、immutable revision 与发布</span></div>
  <div class="record-row"><span class="record-key">RUNTIME</span><span class="record-value"><strong>Docker + runsc (gVisor)</strong></span></div>
  <div class="record-row"><span class="record-key">AI CODING</span><span class="record-value"><strong>Codex & Claude Code</strong> + GPT-5.6、GLM-5.2、DeepSeek-V4-Pro 等</span></div>
  <div class="record-row"><span class="record-key">SLIDES</span><span class="record-value">采用 Slides as Code，由 Markdown 源文件通过 <strong>Marp</strong> 编译</span></div>
</div>
