import type { CommunityDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/community/page.tsx`. Copy
 * moved verbatim from its `isZh` ternaries. The contribution paths and
 * activity links are content and stay in the page.
 */
export const community: CommunityDict = {
  metaTitle: "Community · Codewhale",
  metaDescription:
    "File issues, send pull requests, improve translations, and see who contributed to each Codewhale release.",
  kicker: "International open-source community",
  title: "Build Codewhale with contributors around the world.",
  lede: "The runtime, docs, tests, and translations come from contributors across countries, languages, and platforms. A first contribution does not have to be a feature. A clear bug report, a documentation fix, or a small tested patch counts.",
  fileIssue: "File an issue",
  browsePulls: "Browse pull requests",
  readGuide: "Read the contribution guide",
  pathsTitle: "Start with one small thing.",
  pathsScope:
    "Bug reports, code, tests, docs, translations, and review all help. Pick the one that fits the time you have.",
  recordTitle: "From proposal to release, in the open.",
  recordScope:
    "The activity feed shows recent repository work. The community digest keeps the weekly archive of repository activity. The roadmap separates what shipped from what is still being discussed.",
  creditTitle: "Contributor credit is part of the release record.",
  creditScope:
    "This release includes code, tests, reproductions, and verification from the community. If a maintainer reworks a patch before it lands, the original author stays credited in the commit, the changelog, and the contributor record.",
  creditScopeUnreleased:
    "This version includes code, tests, reproductions, and verification from the community. If a maintainer reworks a patch before it lands, the original author stays credited in the commit, the changelog, and the contributor record.",
  creditLabel: "v{version} release credit",
  creditLabelUnreleased: "v{version} credit (unreleased)",
  mergedTitle: "Merged or adapted contributions",
  helpersTitle: "Reports, reproductions, and verification",
  fullRecord: "Full contributor record",
};
