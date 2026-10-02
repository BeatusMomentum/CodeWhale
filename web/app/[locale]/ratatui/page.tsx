import Link from "next/link";
import { PageHeader, Section } from "@/components/page-header";
import { RatatuiExplorer } from "@/components/ratatui/explorer";
import { getRatatuiCopy } from "@/lib/content/ratatui";
import { buildPageMetadata } from "@/lib/page-meta";
import { readCatalogue } from "@/lib/ratatui/catalogue";

const REPOSITORY = "https://github.com/Hmbown/codewhale-ratatui";
const INSTALL = `[dependencies]
codewhale-ratatui = { git = "https://github.com/Hmbown/codewhale-ratatui" }
ratatui = "0.30.2"`;
const STARTER = `use codewhale_ratatui::{NativeComposer, Paint, Theme};
use ratatui::Frame;

fn draw(frame: &mut Frame) {
    let theme = Theme::detect().tui();
    let composer = NativeComposer::new("Review the changes")
        .focused(true);
    frame.render_widget(composer.themed(&theme), frame.area());
}`;

export const revalidate = 3600;

export async function generateMetadata({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const copy = getRatatuiCopy(locale);
  return buildPageMetadata({
    path: "/ratatui",
    locale,
    title: copy.metaTitle,
    description: copy.metaDescription,
  });
}

export default async function RatatuiPage({ params }: { params: Promise<{ locale: string }> }) {
  const { locale } = await params;
  const copy = getRatatuiCopy(locale);
  const catalogue = await readCatalogue();

  return (
    <div className="ratatui-page">
      <PageHeader
        title={copy.title}
        lede={copy.intro}
        actions={
          <>
            <a href="#ratatui-install" className="btn btn-primary">{copy.install}</a>
            <Link href={REPOSITORY} className="btn btn-secondary">{copy.repository}</Link>
          </>
        }
      />
      <RatatuiExplorer catalogue={catalogue} locale={locale} copy={copy} />
      <div className="page-body">
        <Section id="ratatui-install" title={copy.installationTitle} scope={copy.installationDescription} layout="split">
          <div className="stack">
            <pre tabIndex={0} className="code-block" dir="ltr">{INSTALL}</pre>
            <p className="section-scope">{copy.rustVersion} · {copy.license}</p>
            <p>{copy.usageDescription}</p>
            <pre tabIndex={0} className="code-block" dir="ltr">{STARTER}</pre>
            <div className="actions">
              <Link href={`${REPOSITORY}/blob/main/VIEWS.md`} className="section-link">{copy.viewGuide} →</Link>
              <Link href={`${REPOSITORY}/blob/main/QUALITY.md`} className="section-link">{copy.quality} →</Link>
            </div>
            <p className="section-scope">{copy.tryLocally}</p>
            <pre tabIndex={0} className="code-block" dir="ltr">{"git clone https://github.com/Hmbown/codewhale-ratatui\ncd codewhale-ratatui\ncargo run --example gallery"}</pre>
            <div className="actions">
              <Link href={`${REPOSITORY}/actions`} className="section-link">{copy.checks} →</Link>
              <Link href={`${REPOSITORY}/releases`} className="section-link">{copy.releaseHistory} →</Link>
            </div>
          </div>
        </Section>
      </div>
    </div>
  );
}
