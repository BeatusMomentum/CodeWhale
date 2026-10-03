import { existsSync, readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { redirect } from "next/navigation";
import PricingPage from "../app/[locale]/pricing/page";
import { footerLegalLinks } from "./i18n/links";
import { getChrome } from "./i18n/dictionaries";
import { formatLegalDocumentStatus, LEGAL_DOCUMENTS, PRIVACY_SECTIONS, TERMS_SECTIONS } from "./legal-copy";
import TermsPage from "../app/[locale]/legal/terms/page";
import PrivacyPage from "../app/[locale]/legal/privacy/page";

const webRoot = new URL("../", import.meta.url);
vi.mock("next/navigation", () => ({ redirect: vi.fn() }));

describe("public legal routes and retired pricing", () => {
  it("ships real pages for the URLs that used to 404", () => {
    for (const path of [
      "app/[locale]/legal/terms/page.tsx",
      "app/[locale]/legal/privacy/page.tsx",
      "app/[locale]/privacy/page.tsx",
      "app/[locale]/terms/page.tsx",
    ]) {
      expect(existsSync(new URL(path, webRoot)), path).toBe(true);
    }
  });

  it("aliases /privacy and /terms onto the legal paths instead of inventing a second policy", () => {
    expect(footerLegalLinks("en", getChrome("en")).map((l) => l.href)).toEqual([
      "/en/legal/terms",
      "/en/legal/privacy",
    ]);
    expect(LEGAL_DOCUMENTS.terms).toEqual({ version: "2026-09-07", status: "draft", effectiveAt: null });
    expect(LEGAL_DOCUMENTS.privacy).toEqual({ version: "2026-08-21", status: "effective", effectiveAt: "2026-08-21" });
    expect(formatLegalDocumentStatus("terms", getChrome("en").dateLocale)).toContain("Not yet in effect");
    expect(formatLegalDocumentStatus("terms", getChrome("de").dateLocale)).toContain("7. September 2026");
    expect(TERMS_SECTIONS.some((s) => s.title === "Plans and charges")).toBe(true);
    expect(PRIVACY_SECTIONS.some((s) => s.title === "Retention and deletion")).toBe(true);
  });

  it("pins the single publication authority and fails on vendored metadata drift", () => {
    const pin = JSON.parse(readFileSync(new URL("vendor/legal-documents/PIN.json", webRoot), "utf8"));
    expect(pin.sourceRepository).toBe("Hmbown/codewhale-platform");
    expect(pin.sourceCommit).toMatch(/^[a-f0-9]{40}$/);
    expect(pin.files.map((row: { path: string }) => row.path).sort()).toEqual(["legal-documents.d.ts", "legal-documents.js"]);
    for (const row of pin.files) {
      const data = readFileSync(new URL(`vendor/legal-documents/${row.path}`, webRoot));
      expect(createHash("sha256").update(data).digest("hex")).toBe(row.sha256);
    }
  });

  it("renders English and Chinese draft terms without inventing an effective date", async () => {
    for (const locale of ["en", "zh"]) {
      const html = renderToStaticMarkup(await TermsPage({ params: Promise.resolve({ locale }) }));
      expect(html).toContain('data-legal-status="draft"');
      expect(html).toContain('data-legal-version="2026-09-07"');
      expect(html).not.toContain("data-legal-effective-at");
      expect(html).toContain(locale === "en" ? "Not yet in effect" : "尚未生效");
      const privacy = renderToStaticMarkup(await PrivacyPage({ params: Promise.resolve({ locale }) }));
      expect(privacy).toContain('data-legal-status="effective"');
      expect(privacy).toContain('data-legal-effective-at="2026-08-21"');
    }
  });

  it("takes incoming pricing links to installation in the visitor's locale", async () => {
    for (const locale of ["en", "zh", "pt-BR"]) {
      await PricingPage({ params: Promise.resolve({ locale }) });
      expect(redirect).toHaveBeenLastCalledWith(`/${locale}/install`);
    }
  });
});
