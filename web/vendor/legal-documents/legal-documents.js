/**
 * Publication metadata shared by every Codewhale legal page. This record does
 * not approve legal text or publish a draft; approval and publication require
 * a recorded founder decision before its status can change.
 */
export const LEGAL_DOCUMENTS = Object.freeze({
  terms: Object.freeze({ version: "2026-09-07", status: "draft", effectiveAt: null }),
  privacy: Object.freeze({ version: "2026-08-21", status: "effective", effectiveAt: "2026-08-21" }),
});

/** Render the pinned document's status; a revision date never makes a draft effective. */
export function formatLegalDocumentStatus(documentId, dateLocale = "en-US") {
  if (!Object.hasOwn(LEGAL_DOCUMENTS, documentId)) throw new TypeError("Unknown legal document");
  const document = LEGAL_DOCUMENTS[documentId];
  const date = new Intl.DateTimeFormat(dateLocale, { dateStyle: "long", timeZone: "UTC" })
    .format(new Date(`${document.version}T00:00:00Z`));
  if (dateLocale.toLowerCase().startsWith("zh")) {
    return document.status === "draft"
      ? `草案更新于 ${date}。尚未生效：待批准和发布。`
      : `生效并最近更新于 ${date}。`;
  }
  return document.status === "draft"
    ? `Draft updated ${date}. Not yet in effect: pending approval and publication.`
    : `Effective and last updated ${date}`;
}
