import Link from "next/link";
import { notFound } from "next/navigation";
import { PageHeader } from "@/components/page-header";
import { RatatuiExplorer } from "@/components/ratatui/explorer";
import { getRatatuiCopy } from "@/lib/content/ratatui";
import { buildPageMetadata } from "@/lib/page-meta";
import { readCatalogue } from "@/lib/ratatui/catalogue";

type PageParams = Promise<{ locale: string; component: string }>;

export const revalidate = 3600;

// Render component pages on demand instead of expanding every entry across all locales at build time.
export function generateStaticParams() {
  return [];
}

export async function generateMetadata({ params }: { params: PageParams }) {
  const { locale, component } = await params;
  const catalogue = await readCatalogue();
  const entry = catalogue.entries.find((item) => item.name === component);
  if (!entry) notFound();

  return buildPageMetadata({
    path: `/ratatui/${encodeURIComponent(entry.name)}`,
    locale,
    title: `${entry.title} · Codewhale Ratatui`,
    description: entry.description,
  });
}

export default async function RatatuiComponentPage({ params }: { params: PageParams }) {
  const { locale, component } = await params;
  const catalogue = await readCatalogue();
  const entry = catalogue.entries.find((item) => item.name === component);
  if (!entry) notFound();
  const copy = getRatatuiCopy(locale);

  return (
    <div className="ratatui-page">
      <PageHeader
        title={entry.title}
        lede={entry.description}
        actions={
          <>
            <Link href={`/${locale}/ratatui`} className="btn btn-secondary">{copy.back}</Link>
            <Link href={`/${locale}/ratatui#ratatui-install`} className="section-link">{copy.install} →</Link>
          </>
        }
      />
      <RatatuiExplorer catalogue={catalogue} locale={locale} copy={copy} initialEntry={entry.name} />
    </div>
  );
}
