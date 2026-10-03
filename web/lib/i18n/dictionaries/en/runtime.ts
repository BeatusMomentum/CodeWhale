import type { RuntimeDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/runtime/page.tsx`. Copy
 * moved verbatim from its `isZh` ternaries. The integration descriptions and
 * the trust-boundary items are content and stay in the page.
 */
export const runtime: RuntimeDict = {
  metaTitle: "Runtime & Integrations · Codewhale",
  metaDescription:
    "Codewhale's local Runtime API, HTTP/SSE, baseline ACP stdio adapter, MCP servers, an early VS Code companion, and messaging bridges.",
  kicker: "Runtime & Integrations",
  title: "Runtime and integrations",
  titleAside: "运行时与集成",
  titleAsideLang: "zh",
  lede: "Embed Codewhale in the tools you already use. Beyond the terminal, it runs a local control plane that editors, scripts and chat apps can talk to.",
  integrationsTitle: "Integration surfaces",
  experimental: "Experimental",
  trustTitle: "Trust boundary",
  factsTitle: "Runtime facts",
  version: "Version",
  toolCount: "Tool count",
  sandboxBackends: "Sandbox backends",
  details: "Details",
  sourceRevision: "Source revision",
  docsLead: "Detailed implementation docs:",
  runtimeApiDoc: "Runtime API and ACP stdio adapter",
  mcpDoc: "MCP integration",
};
