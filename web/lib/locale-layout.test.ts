import { describe, expect, it, vi } from "vitest";

vi.mock("next/font/local", () => ({ default: () => ({ variable: "font" }) }));

const { default: LocaleLayout, generateMetadata } = await import("@/app/[locale]/layout");
const { default: RootNotFound } = await import("@/app/not-found");

function render(locale: string) {
  return LocaleLayout({ children: null, params: Promise.resolve({ locale }) });
}

describe("locale layout", () => {
  // Middleware skips dotted paths, so `/wp-login.php` reaches `[locale]`.
  it("answers not-found for a segment that is not a locale", async () => {
    await expect(render("wp-login.php")).rejects.toMatchObject({
      digest: expect.stringContaining("404"),
    });
  });

  // The layout cannot catch its own notFound(), so the root boundary answers.
  // Without app/not-found.tsx that was the framework's bare page, with no
  // document shell, nav, or footer.
  it("answers that not-found inside the default locale's shell", async () => {
    const element = RootNotFound();
    expect(element.type).toBe(LocaleLayout);
    const document = await LocaleLayout(element.props);
    expect(document.type).toBe("html");
    expect(document.props.lang).toBe("en");
  });

  // Otherwise the home page's title, canonical, and hreflang stream into it.
  it("gives that not-found the not-found metadata, not the home page's", async () => {
    const metadata = await generateMetadata({ params: Promise.resolve({ locale: "wp-login.php" }) });
    expect(metadata.title).toBe("Not found · Codewhale");
    expect(metadata.alternates).toEqual({});
  });

  it("renders a real locale", async () => {
    const element = await render("en");
    expect(element.props.lang).toBe("en");
  });
});
