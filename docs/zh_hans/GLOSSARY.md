# Codewhale 中文术语表 / Chinese terminology glossary (draft v1)

Source snapshot: `origin/main` of codewhale, plus `codewhale-ops/CURRENT_DECISIONS.md` §16, §19, §21 (2026-09-29).
Status: proposal for founder/maintainer ratification. Counts were taken with `grep -o` over the stated corpora and are approximate (substring counts, so 代理 also matches 子代理, etc.).

Corpora: **TUI** = `crates/localization/locales/zh-Hans.json` / `zh-Hant.json`; **docs** = `docs/zh_hans/*.md` (49 files); **web** = `web/lib/i18n/dictionaries/zh/*` and `web/gt-catalog/zh.json`.

## 0. Rules that apply to every row

1. **Never translate** (write exactly as in code, in backticks in prose): `Codewhale`, command names (`/fleet`, `codewhale fleet`), config keys, env vars, file paths, flags, key names (Shift+Tab), model names and vendor names, JSON field names, error codes, `{named}` placeholders.
2. **Product marks stay in English verbatim** (VOICE.md / CURRENT_DECISIONS §19: "产品术语必须一字不差"): `Plan`, `Work`, `Operate`, `Ask`, `Auto-Review`, `Full Access`, `Fleet`. In prose docs, add the Chinese gloss once at first mention per document, e.g. `Full Access（完全访问）`, then use the English label. In running TUI sentences where an English label would break the grammar, the zh column term is allowed. Never mix: do not write 自动审查 for Auto-Review.
3. **User-facing copy says "agent" (智能体), never worker / sub-agent / lane** (§16). `worker`, `subagent`, `lane` remain fine as code identifiers and in engineering docs (`SUBAGENTS.md`, `RUNTIME_API.md`) in backticks.
4. Translate the meaning, not the metaphor: do not borrow legal or political words for technical concepts (see Constitution).
5. One English term = one Chinese term inside a document. If two Chinese words are needed, the English concepts differ; add a row here.
6. Hans and Hant are two locales, not a character conversion. Regional vocabulary differs (section 3). A Hant pack must never contain untranslated English where Hans has Chinese (37+19 stray "Constitution/constitution" in zh-Hant today).
7. Space between Latin/digits and CJK is optional but must be consistent within a file (current files are mixed).

## 1. Product vocabulary (CURRENT_DECISIONS §16, §19, §21)

| English | zh-Hans | zh-Hant | Notes |
|---|---|---|---|
| Codewhale | Codewhale | Codewhale | Never translate, never add 鲸/鯨 nicknames in product copy. |
| Agent | 智能体 | 代理 | One AI worker with a role. Hans: 智能体 (docs 72, web 11, TUI 15). Do not use bare 代理 in Hans: it means "proxy" (代理服务器) and 代理 is used 336 times in docs for both meanings. Hant: 代理 is the pack majority (44) and Taiwan usage; 智慧體 (7) and 智能體 (1) are strays. Always write proxy as 代理服务器 / 代理伺服器 or `proxy`. |
| Sub-agent (internal) | 智能体 (user-facing); 子智能体 (engineering docs only) | 代理; 子代理 (engineering docs only) | §16: user-facing says "agent". 子代理 (Hans TUI 18, docs 167) must become 智能体 in UI strings; keep 子智能体 in `SUBAGENTS.md` where the parent/child relation matters. "Stop agent" = 停止智能体 / 停止代理. |
| Worker / lane (internal) | (do not surface) | (do not surface) | TUI currently shows 工作者 (8) and 工作器 (9) for "workers"; replace with 智能体. |
| Fleet | Fleet | Fleet | Keep English (the command is `codewhale fleet`). Gloss once: `Fleet（智能体团队）` / `Fleet（代理團隊）`. Do NOT translate as 团队 (TUI 28, "团队的操作员路由") or 舰队 (5) or 艦隊 (5). Generic "team" in other senses may be 团队. |
| Plan (mode) | Plan | Plan | Verbatim; in prose `Plan 模式` / `Plan 模式`. Codewhale looks and proposes; cannot change files or run commands. Avoid 计划 for the mode (计划 = a plan artifact). |
| Work (mode) | Work | Work | Verbatim; replaces legacy "Act" (7 stray `Act` in zh-Hans TUI, 30 in docs). Do not translate as 工作模式 in UI (TUI 1). |
| Operate (mode) | Operate | Operate | Verbatim. You coordinate a Fleet. |
| Permissions | 权限 | 權限 | Section title. Not 策略/姿态 (posture, policy). |
| Ask | 询问 | 詢問 | Permission level. Label may stay `Ask`. |
| Auto-Review | 自动审核 | 自動審核 | Chosen over 自动审查 (Hans TUI 6 vs 2; Hant 6 vs 2). Docs keep `Auto-Review` in English (29 uses, 0 Chinese). |
| Full Access | 完全访问 | 完整存取 | Hans TUI 完全访问 (14); docs keep English (44) with 2 Chinese uses. Hant TUI is split 完全存取 6 / 完整存取 6; choose 完整存取. "Cycle Access" = 切换访问级别 / 切換存取層級. |
| Coordinator | 协调者 | 協調者 | Your own session when it leads a Fleet. Not 操作员/操作員 (TUI 2, "operator"), not 领队/領隊. |
| Advisor | 顾问 | 顧問 | Second-opinion model. zh-Hant TUI leaves "advisor" in English: translate. |
| Tasks panel | 任务面板 | 任務面板 | §19 replaces Workbar / work bar / work dock. Current TUI: 工作栏 (5), 侧边栏 (sidebar toggle), 待办和工作… Use one word. Todo items inside stay 待办 / 待辦. |
| Workbar | (retired) | (retired) | English UI copy still says "workbar"; treat as a source defect and fix English first. |
| Command / Connected app | 命令 / 已连接的应用 | 指令 / 已連線的應用程式 | Approval categories (shell command / MCP server tool). Not "Bash" or "MCP Read/Action". Hant 指令 vs 命令: pick 指令 and apply everywhere. |
| Allow once | 仅允许本次 | 僅允許一次 | Approval choice. |
| Allow for this conversation | 在本次对话中允许 | 在本次對話中允許 | Conversation, not session, in the label (source string says conversation). |
| Always allow in this repo | 在此仓库中始终允许 | 在此儲存庫中一律允許 | |
| Don't allow | 不允许 | 不允許 | Not 拒绝 for the button (拒绝 is the resulting state: "已拒绝"). |
| Stop | 停止 | 停止 | Ends the current turn. Not 中止/终止 (abort). |
| Making room | 腾出空间 (proposed) | 騰出空間 (proposed) | §19 user-facing label for summarizing earlier conversation to fit the model; forbids "auto-compacting". Current zh strings say 压缩上下文 (25) / 自动压缩. In engineering docs, `compaction` = 压缩. NEEDS native check; do not ship the proposed label without it. |
| Settings | 设置 | 設定 | The preferences screen. Hans TUI 设置 66, 设定 2 (fix). Config file / `config.toml` content = 配置 (see below). |
| Presence: resting · working · needs you · done · offline | 休息中 · 工作中 · 需要你 · 已完成 · 离线 (proposed) | 休息中 · 工作中 · 需要你 · 已完成 · 離線 (proposed) | Whale states; keep short, no exclamation. |
| Thinking: off / high / max | 思考：关 / 高 / 最大 | 思考：關 / 高 / 最大 | Model reasoning effort. §19 says "Thinking", not "Reasoning". Hans TUI has 推理强度 (4) and 推理级别: change to 思考强度. "reasoning model" as a model category may stay 推理模型. Do not translate `max` alone; write "max（最大）" only if a gloss is needed. |
| Constitution | 宪章 | 憲章 | See "Decisions" below. Hans TUI 宪章 (23), 宪法 0 (test-enforced); Hant TUI 憲章 (2), 憲法 (1, defect), and 56 untranslated "Constitution/constitution" (defect). Docs: 宪章 23, 宪法 1 (defect). Never 宪法, 教义, 自由原则, 仓库法则. Never changes permissions. |

## 2. Runtime, engine and platform terms

| English | zh-Hans | zh-Hant | Notes |
|---|---|---|---|
| Engine | 引擎 | 引擎 | The Rust core. Docs 75, consistent. |
| Runtime | 运行时 | 執行階段 | Noun, as in "runtime API". Hant is split 執行時 11 / 執行階段 5 / 執行期 3: pick 執行階段. |
| Session | 会话 | 工作階段 | Persisted unit of work owned by `session_manager`. Hant must not use 會話 (14) or 對話 for session. |
| Conversation | 对话 | 對話 | Dialogue content inside a session. Keep distinct from session (Hans docs: 会话 362, 对话 59). |
| Turn | 回合 | 回合 | One user request plus the agent's work until it stops. Chosen over 轮次/本轮 (TUI 23+16, docs 2): docs 379, web 37, gt 26, Hant 29. Rewrite 本轮 to 本回合; 每轮 to 每回合. |
| Provider | 提供商 | 供應商 | Model provider (DeepSeek, OpenAI, ...). Hans: drop 供应商 (3), 提供方 (4+2), 服务商 (3). Hant is split 供應商 39 / 提供商 33 / 提供者 15: pick 供應商. Provider names stay as is. |
| Route (model route) | 路由 | 路由 | Which provider+model handles work. Hans 路由 docs 443; drop 路线 (TUI 11, web 5, docs 5). Not for URL "routes" in web docs: there also 路由, fine, but add `route` in backticks. |
| Model | 模型 | 模型 | Never 型号. |
| Context window | 上下文窗口 | 上下文視窗 | Hant TUI currently writes 情境 in places ("最大情境…"): use 上下文 in both locales. |
| Compaction / compact | 压缩 | 壓縮 | Engineering term (docs 58). User-facing label is "Making room" above. |
| Token (LLM usage) | token | token | Keep English lowercase for model tokens ("token 用量", "输入 token"). Hans currently mixes 令牌 (TUI 15, docs 59, web 7) and Token (docs 36) and 词元 (4). 令牌 is ambiguous with auth tokens, and users write "token" themselves (issue #743). |
| Token (auth) | 令牌 | 權杖 | Credential tokens only ("访问令牌", "API 令牌"). Hant TUI has 令牌 5 / 權杖 7: use 權杖. |
| Skill | 技能 | 技能 | Consistent. |
| Plugin | 插件 | 外掛 | Hant: 外掛 (44) over 外掛程式 (15) and 插件 (1). "Managed plugin": 托管插件 / 受管外掛. |
| Extension (editor, IDE) | 扩展 | 擴充功能 | Different concept from plugin; do not merge. |
| MCP | MCP | MCP | Never translate. "MCP server" = MCP 服务器 / MCP 伺服器. |
| Hook | 钩子 | 掛鉤 | Hant: 掛鉤 (10) over 鉤子 (3). |
| Workflow | 工作流 | 工作流程 | Hans: 工作流 (docs 79; 1 stray 工作流程). Hant is split 7/7: pick 工作流程. |
| Receipt | 回执 | 回執 | Record proving what happened (docs 168, TUI 5). Web uses 收据 (9) for the same concept (fleet and review receipts): change to 回执. 收据 only for a payment receipt. |
| Approval (noun) | 审批 | 核准 | Hans: 审批 (docs 169, web 40, gt 34) for the noun/flow; 批准 for the verb/state ("需要批准", TUI 21). Hant: 核准 (20) for both; 審批 (7) is a stray. "Authorization" = 授权 / 授權 (distinct; see AUTHORIZATION_ORDER). |
| Sandbox | 沙箱 | 沙箱 | Consistent in all corpora (docs 110, web 33). Never 沙盒. |
| Credential | 凭据 | 憑證 | Hant is split 憑證 17 / 憑據 15: pick 憑證. API key = API 密钥 / API 金鑰. |
| Trust | 信任 | 信任 | |
| Memory (agent notes) | 记忆 | 記憶 | Agent memory feature. RAM = 内存 / 記憶體. |
| Upstream | 上游 | 上游 | |
| Telemetry | 遥测 | 遙測 | |
| Diff | diff | diff | Keep English; Chinese readers use it. |
| Repository / repo | 仓库 | 儲存庫 | |
| Worktree | 工作树 | 工作樹 | Git term. Note: AGENTS.md forbids creating them; docs still describe the feature. |
| Configuration (file, keys) | 配置 | 設定 | Hans: 配置 = config file/keys/structure (docs 567), 设置 = UI screen and preferences (docs 356). Hant collapses both to 設定. |
| Usage (tokens/cost) | 用量 | 用量 | Hans TUI mixes 使用统计 (13) and 用量 (10): pick 用量. |

## 3. Regional vocabulary (Hans vs Hant), apply to every doc

| English | zh-Hans | zh-Hant |
|---|---|---|
| server | 服务器 | 伺服器 |
| file | 文件 | 檔案 |
| project | 项目 | 專案 (Hant TUI mixes 專案 31 and 項目 11) |
| software / program | 软件 / 程序 | 軟體 / 程式 |
| default | 默认 | 預設 |
| network | 网络 | 網路 |
| support | 支持 | 支援 |
| message | 消息 | 訊息 |
| queue | 队列 | 佇列 |
| information | 信息 | 資訊 |
| video, audio | 视频, 音频 | 影片, 音訊 |
| terminal | 终端 | 終端機 |
| repository | 仓库 | 儲存庫 |
| run / execute | 运行 / 执行 | 執行 |

## 4. Inconsistencies found: term, variants seen with counts, chosen

Counts: TUI zh-Hans / docs / web (dictionaries + gt-catalog) unless noted.

| Concept | Variants seen | Chosen | Action |
|---|---|---|---|
| Fleet | Fleet (TUI 118, docs 118, gt 13); 团队 (TUI 28); 舰队 (5); Hant 團隊 (28), 艦隊 (5) | Fleet, gloss once | Replace 团队/舰队/團隊/艦隊 where it means Fleet, e.g. `CmdFleetDescription`, `FleetModelReasonOperatorRoute`. |
| Agent (Hans) | 智能体 (TUI 15, docs 72, web 11); 子代理 (TUI 18, docs 167); 子智能体 (TUI 2, docs 48); 代理 (docs, mixed with proxy) | 智能体; 子智能体 in engineering docs only | Replace 子代理 in UI strings; disambiguate 代理 = proxy. |
| Agent (Hant) | 代理 (44); 子代理 (20); 智慧體 (7); 智能體 (1) | 代理 | Fix the 8 strays. Needs Taiwan native check. |
| Worker | 工作者 (8); 工作器 (9); worker kept English (docs 268) | 智能体 | §16: UI never says worker. Docs keep `worker` in backticks for API/CLI names. |
| Constitution | 宪章 (TUI 23, docs 23, web 5); 宪法 (docs 1); Hant 憲章 (2), 憲法 (1), English "Constitution/constitution" left untranslated (56) | 宪章 / 憲章 | Fix `SetupConstitutionFileLoadedUnselected` in zh-Hant; translate the 56 leftovers; extend the ban test to zh-Hant. Issue #4949 settled by maintainer (test in `crates/tui/src/localization.rs`). |
| Session | 会话 (TUI 141, docs 362, web 79); Hant 工作階段 (126), 會話 (14), 對話 (41) | 会话 / 工作階段 | Hant: rewrite 會話 to 工作階段; keep 對話 for conversation. |
| Turn | 回合 (docs 379, web 37, gt 26, TUI 13); 轮次 (TUI 23, docs 2); 本轮 (TUI 16); 一轮 (5); Hant 回合 (29) vs 輪次 (12) / 本輪 (15) | 回合 | Replace 轮次/本轮/一轮; Hant 輪次/本輪. |
| Provider | 提供商 (TUI 80, docs 288, web 48); 供应商 (3); 提供方 (4 TUI, 2 web); 服务商 (docs 3); Hant 供應商 (39), 提供商 (33), 提供者 (15) | 提供商 / 供應商 | Normalise. |
| Route | 路由 (TUI 41, docs 443); 路线 (TUI 11, web 5, docs 5); Hant 路由 (41), 路線 (11) | 路由 | Replace 路线/路線. |
| Approval | 审批 (TUI 13, docs 169, web 40); 批准 (TUI 21, docs 44, web 11); 授权 (docs 53); 许可 (docs 4); Hant 核准 (20), 審批 (7), 批准 (7) | 审批 (noun) / 批准 (verb, state) ; Hant 核准 | Hant: replace 審批/批准 with 核准. |
| Auto-Review | 自动审核 (TUI 6); 自动审查 (TUI 2, docs 3); English Auto-Review (docs 29); Hant 自動審核 (6), 自動審查 (2) | 自动审核 / 自動審核; keep English label in docs | Replace 审查. |
| Full Access | 完全访问 (TUI 14, docs 2); English (docs 44); Hant 完全存取 (6), 完整存取 (6) | 完全访问 / 完整存取 | Hant: unify to 完整存取. |
| Token | 令牌 (TUI 15, docs 59, web 7); Token/token (TUI 20, docs 36); 词元 (TUI 3, docs 1); Hant 令牌 (5), 權杖 (7), Token (22) | token (model), 令牌/權杖 (auth) | Largest source of ambiguity. Replace 令牌 where it means model tokens. |
| Receipt | 回执 (TUI 5, docs 168); 收据 (web 9, gt 1) | 回执 | Web dictionaries `docs-fleet.ts`, `docs-review.ts`. |
| Settings vs config | 设置 (TUI 66, docs 356, web 71); 设定 (TUI 2, gt 3); 配置 (TUI 83, docs 567, web 36) | 设置 = UI, 配置 = files/keys | Replace 设定. §19 says "Settings" not "Config" in UI. |
| Workbar / Tasks panel | 工作栏 (TUI 5, docs 9, web 7); 侧边栏; 工作台 (docs 2) | 任务面板 | Also fix English source. |
| Thinking / reasoning | 思考 (TUI 12, docs 69); 推理 (TUI 18, docs 95); 推理强度 (4) | 思考 for the effort setting; 推理模型 as category | Replace 推理强度/推理级别. |
| Plugin (Hant) | 外掛 (44); 外掛程式 (15); 插件 (1) | 外掛 | |
| Hook (Hant) | 掛鉤 (10); 鉤子 (3) | 掛鉤 | |
| Workflow (Hant) | 工作流程 (7); 工作流 (7) | 工作流程 | |
| Runtime (Hant) | 執行時 (11); 執行階段 (5); 執行期 (3) | 執行階段 | |
| Credential (Hant) | 憑證 (17); 憑據 (15) | 憑證 | |
| Engine/Runtime (Hans) | 运行时 (TUI 20, docs 324); 运行环境 (0) | 运行时 | Consistent. |
| Sandbox | 沙箱 (all corpora); 沙盒 (0) | 沙箱 | Already consistent. |
| Compaction | 压缩上下文 (TUI 25, docs 58); 自动压缩; Hant 整理 (11, mostly "重新整理" = refresh, unrelated) | 压缩 (engineering); "Making room" label pending | Do not use 整理 for compaction. |
| Mode names | "Act 模式" (TUI 7, docs 30) vs Work (§19); 计划模式 (docs 3); 工作模式 (1) | Plan / Work / Operate verbatim | Act to Work. |
| Coordinator / operator | 协调者 (TUI 2, docs 1); 领队 (1); 操作员 (2 in `FleetModelReasonOperatorRoute`) | 协调者 | Remove 操作员/領隊. |
| Approval-choice labels | 仅允许本次 (1) ; 允许 (TUI 19, docs 87) | per §19 list | Verify against `en.json` once the English labels are final. |

## 5. Process rules for keeping translations accurate

- Every translated doc starts with the English source path and a `last synced with English revision` date (already the convention in `docs/zh_hans/LOCALIZATION.md`). When the English file changes, the sync date must change in the same commit, or the translation is marked stale. Reviewers on PRs #6662/#6663 flagged stale content behind fresh sync dates.
- Machine-assisted drafts must be reviewed against this glossary before merge; a lexicon lint (like the existing warn-only one for §19) should read section 4 and flag banned variants.
- Ask a native reader for zh-Hant. The pack is labelled "waiting for native review" and the Hant choices above are the least validated.
