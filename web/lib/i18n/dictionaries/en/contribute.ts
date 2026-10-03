import type { ContributeDict } from "../types";

/**
 * English reference dictionary for `app/[locale]/contribute/page.tsx`. Copy
 * moved verbatim from its `isZh` ternaries. The contribution paths, workflow
 * steps and review notes are content and stay in the page.
 */
export const contribute: ContributeDict = {
  metaTitle: "Contribute · Codewhale",
  metaDescription:
    "File issues, improve translations and documentation, and send pull requests to the international Codewhale community.",
  kicker: "Contribute to an international open-source project",
  title: "Start with one concrete improvement.",
  lede: "Codewhale welcomes contributors across countries, languages, platforms, and experience levels. Clear bug reports, reproduction results, documentation corrections, translations, and small complete patches all move the project forward.",
  fileIssue: "File an issue",
  browsePulls: "Browse pull requests",
  fullGuide: "Open the full contributor guide",
  pathsTitle: "Choose the contribution that fits.",
  workflowTitle: "From a problem to a reviewable patch.",
  reviewTitle: "Make the change easy to verify.",
  reviewScope:
    "A reviewer needs the problem, the reason for the change, test evidence, and remaining risk. Clear scope usually leads to clearer feedback.",
  devTitle: "Build and run the relevant checks.",
  devScope:
    "The repository uses stable Rust. Run the focused test for your change first, followed by formatting, Clippy, and the workspace suite.",
};
