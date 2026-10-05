import type { HomeDict } from "../types";

/**
 * German home dictionary — native copy for the whale-road landing page,
 * in the current direction: your models, more capable together; agents
 * and control on your own machine; availability stated per surface as it
 * is today. Product vocabulary stays literal (Plan / Work / Operate, Ask /
 * Auto-Review / Full Access, Codewhale, TUI, codewhale exec, Fleet).
 */

export const home: HomeDict = {
  metaTitle: "Codewhale — Entwickle und automatisiere mit den Modellen deiner Wahl",
  metaDescription:
    "Entwickle Software, arbeite mit deinen Dateien und automatisiere alltägliche Aufgaben mit Open-Source-Agenten und gehosteten oder lokalen KI-Modellen deiner Wahl.",
  heroTitle: "Entwickle und automatisiere mit den Modellen deiner Wahl",
  heroIntro:
    "{brand} gibt dir Agenten, die Software entwickeln, mit deinen Dateien arbeiten und wiederkehrende Aufgaben in wiederverwendbare Workflows verwandeln können. Beschreibe, was du erreichen möchtest, und wähle gehostete oder lokale Modelle, die zur Aufgabe passen, wobei du den Anbieter im Laufe der Arbeit wechseln kannst.",
  getCodewhale: "Codewhale holen",
  heroInstallAria: "Installationsbefehl",
  exploreProduct: "Produkt ansehen",
  shotPreview: "Terminal-Vorschau",
  shotBuild: "Entwicklungsbuild v{version}",
  screenshotAlt:
    "Codewhale v{version}, Entwicklungsbuild: Wal, neue Sitzung, Nachrichteneingabe, Ask-Berechtigungen, Work-Modus und Modellstatus. Darstellung der tatsächlichen Ausgabe eines isolierten Terminals.",
  latestRelease: "Aktuellstes Release {tag}",
  releaseUnavailable: "Release-Status nicht verfügbar",
  currentSource: "Quelle",
  sourceCandidate: "Unveröffentlicht",
  publishedRelease: "veröffentlicht",
  figcaptionSourceCandidate: "unveröffentlicht",
  chapterTerminal: "Dein Terminal",
  chapterTerminalTitle: "Beginne mit etwas, das du erstellen möchtest",
  gainHeading:
    "Was du mit Codewhale machen kannst",
  gainLede:
    "Beginne mit einem Projekt, einer Frage oder einer Aufgabe, die du automatisieren möchtest, und arbeite dann mit einem Agenten oder verteile Teile einer größeren Aufgabe auf mehrere.",
  gain: [
    [
      "Entwickle etwas",
      "Beschreibe, was du erstellen möchtest, und arbeite mit Agenten, die deinen Code lesen, Dateien bearbeiten, Befehle ausführen und das Ergebnis prüfen können."
    ],
    [
      "Automatisiere alltägliche Arbeit",
      "Erstelle Skripte und Workflows für wiederkehrende Aufgaben, damit du sie bei Bedarf erneut im Terminal ausführen kannst."
    ],
    [
      "Arbeite mit verschiedenen Modellen",
      "Nutze gehostete oder lokale Modelle für deine Agenten und setze unterschiedliche Modelle und Rollen für die Teile einer Aufgabe ein, zu denen sie passen."
    ]
  ],
  chapterModels: "Deine Modelle",
  modelsHeading: "Eine Auswahl an Modellen für jede Aufgabe",
  modelsBody:
    "Verbinde dich direkt mit einem Anbieter gehosteter Modelle, greife über ein Gateway auf mehrere Anbieter zu oder führe ein Modell lokal aus, und wähle während der Arbeit das Modell für jede Sitzung.",
  modelsFacts: [
    ["Gehostet", "Dein eigener API-Schlüssel, gespeichert mit codewhale auth set --provider <id>"],
    ["Gateway", "Ein Endpoint für viele Modelle, den Provider wählst weiterhin du"],
    ["Lokal", "vLLM, SGLang, Ollama auf localhost — meist ohne Schlüssel"],
  ],
  modelsLink: "Modelle und Anbieter entdecken",
  startHeading: "Erste Schritte mit Codewhale",
  startLede:
    "Sobald du Codewhale installiert und ein Modell verbunden hast, kannst du deine erste Aufgabe im Terminal beschreiben und Fleet hinzunehmen, wenn mehrere Agenten die Arbeit unter sich aufteilen sollen.",
  startGuideLink: "Leitfaden für die ersten Schritte lesen",
  startVocabularyLink: "Produktvokabular ansehen",
  chapterAvailability: "Wo es läuft",
  availabilityHeading: "Wo du Codewhale nutzen kannst",
  availabilityLede:
    "Das Terminal, der lokale Browser-Client und die von der Community gepflegte CodeWhale GUI sind schon heute verfügbar. Die Desktop-App wird zum wichtigsten Codewhale-Client, und die gehostete Web-App wird neu aufgebaut, damit sie dazu passt; beide teilen dasselbe Sitzungsmodell, und ihre Verfügbarkeit ist unten aufgeführt.",
  availability: [
    [
      "Terminal",
      "Veröffentlicht",
      "Binärdateien aus den GitHub-Releases für Linux, macOS und Windows; npm und Cargo sind Alternativen. Android unter Termux ist eine Vorschau."
    ],
    [
      "CodeWhale GUI (VS Code)",
      "Verfügbar",
      "Grafische Oberfläche aus der Community, in einem eigenen Projekt: Chat, Threads und Dateiänderungen in einer VS Code-Seitenleiste über dieselbe Runtime. Installiere sie über den VS Code Marketplace.",
      "https://marketplace.visualstudio.com/items?itemName=HengQuWorld.brotherwhale-vscode"
    ],
    [
      "Web-App",
      "Entwicklungsvorschau",
      "Wird neu aufgebaut, damit sie zur Desktop-App passt. Heute kannst du dich anmelden und dann in einer laufenden Terminal-Sitzung /rc eingeben, um sie im Web fortzusetzen. Die Ausführung gehosteter Aufgaben wird noch qualifiziert."
    ],
    [
      "Desktop-App",
      "Entwicklungsbuild",
      "Die native App, die zum wichtigsten Codewhale-Client wird: Ordner, Unterhaltungen und Modellverbindungen in einem Desktop-Fenster. Ein öffentlicher Download folgt später."
    ],
    [
      "Cloud-Computer",
      "In Entwicklung",
      "Gehostete Computer zum Ausführen deiner Aufgaben."
    ]
  ],
  availabilityNote:
    "Du kannst das Terminal und die GUI ohne Codewhale-Konto nutzen, und die Nutzung gehosteter Modelle rechnet dein Anbieter ab.",
  accountLink: "Konto erstellen",
  surfacesHeading: "Möglichkeiten, mit Codewhale zu arbeiten",
  surfaces: [
    ["TUI", "Interaktive Arbeit im Terminal"],
    ["codewhale exec", "Skripte und CI"],
    ["Lokaler Web-Client","Oberfläche auf localhost; gehostete Arbeitsumgebung im Browser in Entwicklung"],
    ["Runtime API + MCP", "Lokale Integrationen"],
    ["Fleet","Mehrere Agenten für dieselbe Aufgabe"],
  ],
  runtimeLink: "Integrationen entdecken",
  installBandHeading: "Installiere Codewhale auf macOS oder Linux",
  copy: "Kopieren",
  copied: "Kopiert ✓",
  binaries: "Binärdateien",
  chinaMirrors: "China-Mirrors",
  installGuideLink: "Installationsleitfaden lesen",
  communityHeading: "Hilf mit, Codewhale zu verbessern",
  communityBody:
    "Ob du einen Fehler gefunden hast, eine Idee für eine Funktion hast oder deinen ersten Pull Request einreichen möchtest: Wir möchten von dir hören und gemeinsam an der weiteren Entwicklung arbeiten.",
  communityLinksAria: "Community-Links",
  contribute: "Pull Request senden",
};
