import type { HomeDict } from "../types";

/**
 * Catalan home dictionary — native copy for the whale-road landing page,
 * in the current direction: your models, more capable together; agents
 * and control on your own machine; availability stated per surface as it
 * is today. Product vocabulary stays literal (Plan / Work / Operate, Ask /
 * Auto-Review / Full Access, Codewhale, TUI, codewhale exec, Fleet).
 */

export const home: HomeDict = {
  metaTitle: "Codewhale — Crea i automatitza amb els models que triïs",
  metaDescription:
    "Crea programari, treballa amb els teus fitxers i automatitza les tasques quotidianes amb agents de codi obert i els models d’IA allotjats o locals que triïs.",
  heroTitle: "Crea i automatitza amb els models que triïs",
  heroIntro:
    "{brand} et proporciona agents que poden crear programari, treballar amb els teus fitxers i convertir les tasques repetitives en fluxos de treball reutilitzables. Digues-los què vols aconseguir i tria els models allotjats o locals adequats per a la feina, amb la llibertat de canviar de proveïdor sobre la marxa.",
  getCodewhale: "Obtenir Codewhale",
  heroInstallAria: "Ordre d'instal·lació",
  exploreProduct: "Explorar el producte",
  shotPreview: "Vista prèvia del terminal",
  shotBuild: "build de desenvolupament v{version}",
  screenshotAlt:
    "Codewhale v{version}, versió de desenvolupament: balena, sessió nova, camp de missatge, permisos Ask, mode Work i estat del model. Representació de la sortida real d’un terminal aïllat.",
  latestRelease: "Última versió {tag}",
  releaseUnavailable: "Estat de la versió no disponible",
  currentSource: "Font",
  sourceCandidate: "Sense publicar",
  publishedRelease: "publicada",
  figcaptionSourceCandidate: "sense publicar",
  chapterTerminal: "El teu terminal",
  chapterTerminalTitle: "Comença amb alguna cosa que vulguis crear",
  gainHeading:
    "Què pots fer amb Codewhale",
  gainLede:
    "Comença amb un projecte, una pregunta o una tasca que vulguis automatitzar, i després treballa amb un agent o reparteix les parts d’una feina més gran entre diversos.",
  gain: [
    [
      "Crea alguna cosa",
      "Descriu què vols crear i treballa amb agents que poden llegir el teu codi, editar fitxers, executar ordres i comprovar el resultat."
    ],
    [
      "Automatitza la feina quotidiana",
      "Crea scripts i fluxos de treball per a les tasques que repeteixes, de manera que els puguis tornar a executar des del terminal sempre que els necessitis."
    ],
    [
      "Treballa amb models diferents",
      "Fes servir models allotjats o locals per als teus agents, amb models i rols diferents que s’encarreguin de les parts de la feina per a les quals són adequats."
    ]
  ],
  chapterModels: "Els teus models",
  modelsHeading: "Opcions de models per a cada tasca",
  modelsBody:
    "Connecta’t directament a un proveïdor de models allotjats, fes servir una passarel·la per accedir a diversos proveïdors o executa un model en local, i tria quin model fa servir cada sessió mentre treballes.",
  modelsFacts: [
    ["Allotjat", "La teva pròpia clau d’API, desada amb codewhale auth set --provider <id>"],
    ["Gateway", "Un endpoint per a molts models; el proveïdor el segueixes triant tu"],
    ["Local", "vLLM, SGLang, Ollama a localhost; normalment sense clau"],
  ],
  modelsLink: "Explora els models i els proveïdors",
  startHeading: "Primers passos amb Codewhale",
  startLede:
    "Un cop hagis instal·lat Codewhale i connectat un model, pots descriure la teva primera tasca al terminal i afegir un Fleet quan vulguis repartir la feina entre diversos agents.",
  startGuideLink: "Llegeix la guia d’inici",
  startVocabularyLink: "Consulta el vocabulari del producte",
  chapterAvailability: "On funciona",
  availabilityHeading: "On pots fer servir Codewhale",
  availabilityLede:
    "Ja pots fer servir Codewhale al terminal, al navegador local i amb la interfície CodeWhale GUI mantinguda per la comunitat. L’aplicació d’escriptori s’està convertint en el client principal de Codewhale i l’aplicació web allotjada s’està reconstruint per igualar-la; totes dues comparteixen el mateix model de sessió, i la seva disponibilitat s’indica a continuació.",
  availability: [
    [
      "Terminal",
      "Publicat",
      "Binaris de les versions publicades a GitHub per a Linux, macOS i Windows; npm i Cargo són alternatives. Android amb Termux és una vista prèvia."
    ],
    [
      "CodeWhale GUI (VS Code)",
      "Disponible",
      "Interfície gràfica mantinguda per la comunitat, en un projecte a part: xat, fils i canvis de fitxers en una barra lateral del VS Code sobre el mateix Runtime. Instal·la-la des del VS Code Marketplace.",
      "https://marketplace.visualstudio.com/items?itemName=HengQuWorld.brotherwhale-vscode"
    ],
    [
      "Aplicació web",
      "Vista prèvia de desenvolupament",
      "S’està reconstruint per igualar l’aplicació d’escriptori. Avui pots iniciar sessió i després escriure /rc en una sessió de terminal en execució per continuar-la des del web. L’execució de tasques allotjades encara s’està validant."
    ],
    [
      "Escriptori",
      "Build de desenvolupament",
      "L’aplicació nativa que s’està convertint en el client principal de Codewhale: carpetes, converses i connexions de models en una sola finestra d’escriptori. La descàrrega pública arribarà més endavant."
    ],
    [
      "Ordinadors al núvol",
      "En desenvolupament",
      "Ordinadors allotjats per executar les teves tasques."
    ]
  ],
  availabilityNote:
    "Pots fer servir el terminal i la interfície gràfica sense un compte de Codewhale, i el teu proveïdor factura qualsevol ús de models allotjats.",
  accountLink: "Crear un compte",
  surfacesHeading: "Maneres de treballar amb Codewhale",
  surfaces: [
    ["TUI", "Treball interactiu al terminal"],
    ["codewhale exec", "Scripts i CI"],
    ["Client web local","Interfície a localhost; espai de treball web allotjat en desenvolupament"],
    ["Runtime API + MCP", "Integracions locals"],
    ["Fleet","Diversos agents en una mateixa feina"],
  ],
  runtimeLink: "Explora les integracions",
  installBandHeading: "Instal·la Codewhale a macOS o Linux",
  copy: "Copia",
  copied: "Copiat ✓",
  binaries: "Binaris",
  chinaMirrors: "Mirrors a la Xina",
  installGuideLink: "Llegeix la guia d’instal·lació",
  communityHeading: "Ajuda a millorar Codewhale",
  communityBody:
    "Tant si has trobat un error com si tens una idea per a una funció o vols enviar el teu primer pull request, ens agradaria escoltar-te i treballar plegats en els pròxims passos.",
  communityLinksAria: "Enllaços de la comunitat",
  contribute: "Enviar un pull request",
};
