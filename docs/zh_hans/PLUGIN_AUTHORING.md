# 编写你的第一个 Codewhale 插件

> 英文原文：[PLUGIN_AUTHORING.md](../PLUGIN_AUTHORING.md)。
> 最后与英文同步日期（last synced with English revision）：2026-09-09。

先从一个 skill 开始：把 Markdown 指令文件放进一个小型插件包。
[hello-codewhale 示例](../examples/plugins/hello-codewhale/plugin.json)
只有两个文件，不声明服务器或 hook，也不要求调用工具。
本指南带你从源文件开始，完成审查、启用和调用。

## 1. 创建插件包

直接使用仓库中的示例，或在已安装插件目录之外创建以下目录：

```text
hello-codewhale/
├── plugin.json
└── skills/
    └── hello/
        └── SKILL.md
```

`plugin.json`：

```json
{
  "$schema": "https://agent-plugins.org/schemas/plugin.json",
  "name": "hello-codewhale",
  "version": "0.1.0",
  "description": "A minimal, explicitly invoked greeting skill."
}
```

`skills/hello/SKILL.md`：

```markdown
---
name: hello
description: Greet the user when they explicitly try the hello-codewhale example.
invocation: explicit-only
---

Respond with one short greeting in the user's language. Include the exact text
`hello-codewhale:hello` so they can identify the example they invoked.

Use only the conversation. Do not call tools, run commands, read or write files,
or contact external services.
```

这里保留与仓库一致的英文示例；指令要求用用户的语言打招呼，并包含
`hello-codewhale:hello`，同时不调用工具、不运行命令、不读写文件、不联系外部服务。
Codewhale 会自动发现 `skills/`。`explicit-only` 表示此示例不进入模型的自动
skill 目录，而是由你按名称加载。调用元数据见
[Skills](./SKILLS.md#调用与别名元数据)，清单格式以
[插件包契约](./PLUGIN_BUNDLES.md#清单)为准。
新原生插件使用 `plugin.json` 即可，无需再写另一份清单。

## 2. 安装、检查并信任

在仓库根目录启动 Codewhale，然后在 **Codewhale 会话内**逐条输入：

```text
/plugin install ./docs/examples/plugins/hello-codewhale
/plugin validate hello-codewhale
/plugin show hello-codewhale
```

如果使用自己的插件包，把安装路径替换为对应目录。安装会将插件复制到
`~/.codewhale/plugins/hello-codewhale/`，初始状态为未启用、未信任。
审查已安装的源文件、skill 清单和权限。此示例应只声明 Skills，
没有 MCP 服务器、hook 或申请访问的网络主机。

安装审查会打印一条包含两个完整哈希的命令：

```text
/plugin trust hello-codewhale <full-content-sha256>.<full-capability-sha256>
```

审查后执行实际打印的完整命令；上面的尖括号内容只是占位符。
需要重新查看审查内容时，输入不带令牌的 `/plugin trust hello-codewhale`。
信任操作记录已审查的内容哈希和能力哈希，并创建运行时快照，尚不会启用插件。

```text
/plugin enable hello-codewhale
/skills hello-codewhale:
/skills inspect
```

skill 的名称是 `hello-codewhale:hello`，即插件名加 skill 名。
`/skills inspect` 会标明它来自已审查的插件快照。

## 3. 调用并停用

```text
/skill hello-codewhale:hello
```

Codewhale 确认激活后，再发送普通消息 `请打个招呼。`。
回复应是一句简短问候，并包含 `hello-codewhale:hello`。
示例只提供指令；回复仍使用你选择的模型及其正常供应商连接。
本地安装、审查和激活步骤本身不需要调用模型。

```text
/plugin disable hello-codewhale
```

停用会移除插件提供的功能，但保留信任记录。之后执行
`/skill hello-codewhale:hello` 不应再激活该 skill。
只要已审查的哈希仍然匹配，就可以在需要时重新启用。

## 4. 修改并重新审查

已安装的插件是副本。修改示例的原始源文件不会更新该副本。
要尝试修改后的本地源文件，先停用并卸载已安装的示例，再重新安装源目录：

```text
/plugin disable hello-codewhale
/plugin uninstall hello-codewhale
/plugin install ./docs/examples/plugins/hello-codewhale
/plugin validate hello-codewhale
```

卸载只删除已安装的副本，保留原始示例源文件。审查新的令牌，确认信任后再启用。
从远程来源安装的插件使用 `/plugin update <name>`；详情见
[插件安装指南](./PLUGINS.md#更新与卸载)。

直接修改已发现插件目录内的文件后，使用 `/plugin reload` 刷新注册表。
即使版本号没变，内容变化也会使旧信任记录失效。重新加载不会授予信任。
要主动撤销信任，使用 `/plugin revoke <name>`。

## 只添加需要的组件

所有组件都使用同一套插件审查流程和现有 Codewhale 运行时：

| 组件 | 编写方式 |
| --- | --- |
| Skills | `skills/<name>/SKILL.md`；参见[指令与调用契约](./SKILLS.md)。 |
| MCP | 与清单同级的 `mcp.json`；参见[插件传输与凭据规则](./PLUGIN_BUNDLES.md#校验两种格式)。 |
| Commands | Markdown 命令文件；参见[命令元数据](../architecture/command-dispatch.md#user-commands)。 |
| Agent profiles | Fleet TOML 配置文件；参见[Fleet 编写指南](./FLEET.md#编写-agent-配置fleet-setup)。 |
| Hooks | `HooksConfig` TOML 文件；参见[事件与进程行为](./HOOKS.md)。 |

Commands、Agents 和 Hooks 的路径声明放在 `plugin.json` 的
`extensions["net.codewhale"]` 中，详见
[插件组件契约](./PLUGIN_BUNDLES.md#生效与未生效的组件面)。
不要把 MCP 服务器字段或任意运行时入口放在清单根级。
LSP 和原生扩展可以列入清单，但目前没有可执行的插件适配器。

插件信任**不是操作系统沙箱**。本地 MCP 服务器或 hook 可以启动进程；
启用前需审查其代码和权限。Skills 不授予权限：仓库指令、权限规则、沙箱策略
和工具审批仍然适用。不要在插件包或命令参数中保存凭据。
MCP 使用[插件验证契约](./PLUGIN_BUNDLES.md#校验两种格式)规定的、
经过审查的环境变量引用；添加 hook 前，请阅读独立的
[hook 环境契约](./HOOKS.md#hook-进程环境)。

## 转换现有插件

[`scripts/convert-plugin.py`](../../scripts/convert-plugin.py) 将明确选定的远程 MCP
声明、已打包的本地 Node MCP 服务器和可移植 Skills 转换为原生插件包。需要 Python 3.10+ 和 PyYAML 6+；
如果缺少，请单独安装。转换器不会安装依赖、扫描现有应用配置或凭据、
发送网络请求，也不会执行源代码。

### OpenCode

将以下纯 JSON 保存为 `opencode-mcp.json`：

```json
{
  "mcp": {
    "docs": {
      "type": "remote",
      "url": "https://example.invalid/mcp",
      "oauth": false,
      "enabled": false
    }
  }
}
```

在 Codewhale 仓库根目录的 shell 中运行：

```sh
python3 scripts/convert-plugin.py --format opencode-v1 \
  --config ./opencode-mcp.json --name migrated-tools --output ./migrated-opencode
```

如果数据采用 `mcp.servers.<name>` 结构，请选择 `--format opencode-v2`；
该格式的服务器开关是 `disabled`，不是 `enabled`。根据数据选择格式，
不要依据文件名或上游分支名推断版本。两种格式的远程服务器都要求明确设置 `oauth: false`。
远程 MCP 输出**仅使用 Streamable HTTP**，不会复现 OpenCode 回退到旧版 SSE 的行为。
对于仅支持 SSE 的端点，请手工编写包含 `type: "sse"` 的原生 `mcp.json`，
并使用相同的审查流程。

选定 MCP 服务器时，配置中若包含 `tools`、`permission`/`permissions`、
`agent`/`agents`、旧版 `mode` 或 `default_agent`，转换将被拒绝。
这些设置可能在服务器开关之外进一步限制工具访问。请先在 Codewhale 中手工保留
这些限制，再提供只含 MCP 声明的输入；直接删除这些设置可能扩大访问权限。

转换器不接受 JSONC 注释或末尾多余逗号；请提供只含待移植声明的纯 JSON 副本。

### DeepSeek Harness（DSH）

将以下静态 Cordis 条目列表保存为 `dsh-mcp.yml`：

```yaml
- name: '@deepseek-ai/dsh-mcp-client'
  disabled: true
  config:
    serverName: docs
    transport: streamable-http
    url: https://example.invalid/mcp
```

```sh
python3 scripts/convert-plugin.py --format dsh \
  --config ./dsh-mcp.yml --name migrated-dsh --output ./migrated-dsh
```

DSH 输入也可以使用 JSON，但必须是普通条目列表，不能是完整 profile 或
patch 组合。每个条目的 `name` 都必须为 `@deepseek-ai/dsh-mcp-client`。

### 本地 Node MCP 服务器

对于已打包的 Node MCP 服务器，使用 `--stdio-root SERVER=DIRECTORY` 明确选择
原进程的工作目录。转换器将整个目录复制到 `mcp/<server>`，并把原生服务器的
工作目录设为经审查的副本，保留相对入口导入和只读资源的目录布局。

```json
{
  "mcp": {
    "localdocs": {
      "type": "local",
      "command": ["node", "server.mjs"],
      "environment": {"API_TOKEN": "{env:LOCALDOCS_TOKEN}"},
      "enabled": false
    }
  }
}
```

```sh
python3 scripts/convert-plugin.py --format opencode-v1 \
  --config ./local-mcp.json --stdio-root localdocs=./packaged-localdocs \
  --name local-tools --output ./migrated-local
```

每个本地服务器都必须提供对应的 `--stdio-root`。OpenCode v2 使用 `mcp.servers`
和 `disabled`。静态 DSH 条目使用 `transport: stdio`、`command: node`、
`args: [server.mjs]`；DSH 的 `env` 必须省略或为空，其字面量和表达式不按
OpenCode 环境引用解释。DSH/v2 的 `cwd` 只能省略、为空或为 `.`；选定目录明确
指定原进程的工作目录。只有远程 OpenCode 服务器需要 `oauth: false`。

支持 `node` 加一个相对 `.mjs`、`.js` 或 `.cjs` 入口。原生启动适配器保留包的模块类型
和同级模块导入。TypeScript 需先编译为 JavaScript；转换器不会运行编译器。依赖和只读资源必须
预先打包在该目录内；转换时不运行包管理器、安装脚本、模块加载器或服务器。
符号链接/reparse point、硬链接文件、隐藏文件或目录（包括 `.gitignore`、
`.env*`、`.npmrc` 和 `node_modules/.bin`）、常见凭据文件名及私钥容器会被拒绝。
请准备干净的打包目录；不会根据忽略规则静默遗漏文件。转换前检查每个选定文件
是否嵌入凭据。整个输出仍受 4,096 个文件 / 64 MiB 限制。

Shell 启动器、Node 标志、额外参数、其他解释器、环境变量字面值以及改变模块
加载方式的环境变量名均不支持。写入工作目录、依赖实时工作区或导入包外文件的
服务器需要手工移植。复制文件不等于静态验证 JavaScript 依赖闭包，也不提供
代码沙箱。本地 MCP 以宿主用户权限运行；远程端点的主机声明不会限制本地进程
的网络或文件访问。Codewhale 启动服务器前仍需完成原生安装、能力审查、哈希绑定
信任和启用流程。此功能是已打包 Node MCP 子集，不执行 DSH/Cordis 插件模块。

### 审查转换结果

两个示例都保留服务器的停用状态，并使用占位端点。准备连接时，先替换源文件中的
端点并修改启用开关，再重新转换。输出目录必须尚不存在，且其父目录已存在。
已有输出会被拒绝覆盖；输入被拒绝时不会留下输出插件包。

添加 `--skill ./my-skill` 可选择包含 `SKILL.md` 的目录，也可以用
`--skill ./my-skill.md` 选择单个文件；重复该选项可选择多个 skill。
仅转换 Skills 时可以省略 `--config`。Skills 的 frontmatter 必须包含
`name` 和 `description`。`disable-model-invocation: true` 会转换为原生的
`invocation: explicit-only`。说明性字段 `license`、`compatibility` 和
`metadata` 会保存在伴随数据文件 `SOURCE_SKILL_METADATA.json` 中。
所选 skill 目录中的其他文件会作为数据复制；加载 skill 前需审查这些文件及其指令。

只有完全符合 `{env:MCP_TOKEN}` 形式的 OpenCode 请求头引用会转换为原生
`env_headers`；转换器不会读取变量值。字面量请求头、DSH 请求头表达式以及
URL 中的文件或环境变量替换都会被拒绝。配置的超时值以毫秒表示，
必须是 `1000` 到 `3600000` 之间的整秒值。未指定的超时使用 Codewhale 的默认值。

外部运行时的可执行插件和 hooks、其他 stdio 启动方式、自动 OAuth、配置中的
JavaScript、YAML 别名或标签、
`__jsExpr`，以及不支持的 skill 运行时字段（包括 `user-invocable: false`）
需要手工移植。转换不会复现其他客户端的运行时，也不会绕过 Codewhale 的凭据
和沙箱规则。

阅读生成的 `CONVERSION.md`、`plugin.json`、`mcp.json`（如有）和所有选定的
skill 文件。然后使用 `/plugin install ./migrated-opencode`（或 DSH 输出路径）、
`/plugin validate <name>`，并按前文相同的哈希绑定流程审查、信任和启用。
转换本身既不证明连接可用，也不证明运行时兼容；输出尚未安装、信任或启用。

源码核查日期：2026-09-08。依据 OpenCode `d6855b6b47` 的
[v1 MCP 文档](https://github.com/anomalyco/opencode/blob/d6855b6b47a8433462ac6aeeba882ccf734cb7f1/packages/web/src/content/docs/mcp-servers.mdx)
和 [v2 MCP schema](https://github.com/anomalyco/opencode/blob/d6855b6b47a8433462ac6aeeba882ccf734cb7f1/packages/core/src/config/mcp.ts)，
以及 DSH `c389f96bf3` 的 [MCP 客户端参考](https://github.com/deepseek-ai/deepseek-harness/blob/c389f96bf3a9b6807cb71ed6bdad5849be0df6d8/packages/mcp/mcp-client/README.md)。
上游支持的功能多于此转换器明确支持的范围。

## 社区背景

本指南回应了 [giancarlocp 在讨论 #5827 中提出的插件编写指南与 OpenCode
转换需求](https://github.com/Hmbown/Codewhale/discussions/5827)。
简体中文版本遵循 [SparkofSpike 在 issue #5482 中提出的中文文档工作方向](https://github.com/Hmbown/Codewhale/issues/5482)。
