import http from "node:http";
import { standInPort } from "./shard";

// A stand-in for DataForSEO's keyword API, for the browser suite: it knows
// France in French and the United States in English, and gives a seed five
// keywords made from it. A seed that says "refused" is refused the way an
// account out of funds is, one that says "unavailable" fails the way
// DataForSEO's own servers do, and one that says "nothing" has no keywords.
// A search for a keyword gives a first page of an ad and three sites: two
// every search finds, and one of the keyword's own.
// config/e2e.toml points the dashboard at it, and scripts/e2e.sh each next
// server 100 ports further on.

export const DATAFORSEO_PORT = Number(process.env.E2E_DATAFORSEO_PORT ?? standInPort(12116));

/// The login config/e2e.toml gives the dashboard.
export const DATAFORSEO_LOGIN = "keywords-e2e@example.com";
export const DATAFORSEO_PASSWORD = "dataforseo-e2e-s3cret";

/// One paid call as the dashboard sent it.
export type KeywordCall = {
  keyword: string;
  locationCode: number;
  languageCode: string;
};

const LOCATIONS = [
  { code: 2250, name: "France", iso: "FR", language: ["French", "fr"] },
  { code: 2840, name: "United States", iso: "US", language: ["English", "en"] },
];

/// The keywords the stand-in gives for `seed`, the seed's own row first: what
/// a spec expects to read on the page.
export function keywordsFor(seed: string) {
  const row = (keyword: string, volume: number, cpc: number | null, difficulty: number | null, intent: string) => ({
    keyword,
    volume,
    cpc,
    difficulty,
    intent,
  });
  return [
    row(seed, 12100, 0.42, 38, "commercial"),
    row(`${seed} pas cher`, 5400, 0.31, 14, "transactional"),
    row(`meilleur ${seed}`, 2900, 0.55, 22, "commercial"),
    row(`${seed} avis`, 880, 0.12, 9, "informational"),
    // Nobody bids on it and its difficulty was never worked out.
    row(`fabriquer un ${seed}`, 320, null, null, "informational"),
  ];
}

/// The sites the stand-in finds for a search of `keyword`, in their order,
/// as DataForSEO names them: what a spec expects to read on the page,
/// without the `www.`.
export function sitesFor(keyword: string) {
  const own = keyword
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return ["www.amazon.fr", "www.zooplus.fr", `${own}.example`];
}

function item(row: ReturnType<typeof keywordsFor>[number]) {
  return {
    se_type: "google",
    keyword: row.keyword,
    keyword_info: { search_volume: row.volume, cpc: row.cpc },
    keyword_properties: { keyword_difficulty: row.difficulty },
    search_intent_info: { main_intent: row.intent },
  };
}

/// DataForSEO's envelope around one task.
function envelope(taskCode: number, taskMessage: string, cost: number, result: unknown[] | null) {
  return {
    version: "0.1.20260101",
    status_code: 20000,
    status_message: "Ok.",
    cost,
    tasks_count: 1,
    tasks_error: taskCode === 20000 ? 0 : 1,
    tasks: [
      {
        id: "00000000-0000-0000-0000-000000000000",
        status_code: taskCode,
        status_message: taskMessage,
        cost,
        result_count: result?.length ?? 0,
        result,
      },
    ],
  };
}

export class FakeDataforseo {
  readonly calls: KeywordCall[] = [];
  /// The paid searches, as the dashboard sent them.
  readonly searches: KeywordCall[] = [];
  private server?: http.Server;

  async start() {
    const login = Buffer.from(`${DATAFORSEO_LOGIN}:${DATAFORSEO_PASSWORD}`).toString("base64");
    this.server = http.createServer((req, res) => {
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", () => {
        const send = (status: number, answer: unknown) => {
          res.writeHead(status, { "content-type": "application/json" });
          res.end(JSON.stringify(answer));
        };
        if (req.headers.authorization !== `Basic ${login}`) {
          send(401, {
            status_code: 40100,
            status_message: "You are not authorized to access this resource.",
            cost: 0,
            tasks: null,
          });
          return;
        }
        const path = (req.url ?? "").split("?")[0];
        if (req.method === "GET" && path === "/v3/dataforseo_labs/locations_and_languages") {
          send(
            200,
            envelope(
              20000,
              "Ok.",
              0,
              LOCATIONS.map((location) => ({
                location_code: location.code,
                location_name: location.name,
                location_code_parent: null,
                country_iso_code: location.iso,
                location_type: "Country",
                available_languages: [
                  {
                    available_sources: ["google", "bing"],
                    language_name: location.language[0],
                    language_code: location.language[1],
                    keywords: 1000000,
                    serps: 100000,
                  },
                ],
              })),
            ),
          );
          return;
        }
        if (req.method === "POST" && path === "/v3/serp/google/organic/live/regular") {
          const [search] = JSON.parse(body || "[]") as {
            keyword?: string;
            location_code?: number;
            language_code?: string;
          }[];
          const keyword = search?.keyword ?? "";
          this.searches.push({
            keyword,
            locationCode: search?.location_code ?? 0,
            languageCode: search?.language_code ?? "",
          });
          const organic = sitesFor(keyword).map((domain, at) => ({
            type: "organic",
            rank_group: at + 1,
            rank_absolute: at + 2,
            page: 1,
            // As the real one answers since 2026-11 (#791): the search engine's
            // own address, and the site at the start of the breadcrumb.
            domain: "google.com",
            title: `${keyword} - ${domain}`,
            description: null,
            url: `https://google.com/goto?url=CAES${at}&gsessionid=e2e`,
            breadcrumb: `https://${domain} › ${keyword}`,
          }));
          send(
            200,
            envelope(20000, "Ok.", 0.002, [
              {
                keyword,
                type: "organic",
                se_domain: "google.fr",
                location_code: search?.location_code ?? null,
                language_code: search?.language_code ?? null,
                item_types: ["paid", "organic"],
                items_count: organic.length + 1,
                items: [
                  {
                    type: "paid",
                    rank_group: 1,
                    rank_absolute: 1,
                    domain: "www.ads.example",
                    title: "An ad",
                    url: "https://www.ads.example/",
                  },
                  ...organic,
                ],
              },
            ]),
          );
          return;
        }
        if (req.method !== "POST" || path !== "/v3/dataforseo_labs/google/keyword_suggestions/live") {
          send(404, { status_code: 40400, status_message: "Not Found.", cost: 0, tasks: null });
          return;
        }
        const [task] = JSON.parse(body || "[]") as {
          keyword?: string;
          location_code?: number;
          language_code?: string;
          limit?: number;
        }[];
        const seed = task?.keyword ?? "";
        this.calls.push({
          keyword: seed,
          locationCode: task?.location_code ?? 0,
          languageCode: task?.language_code ?? "",
        });
        if (seed.includes("refused")) {
          send(
            200,
            envelope(40210, "Insufficient Funds. Your account's balance is too low to complete this request.", 0, null),
          );
          return;
        }
        if (seed.includes("unavailable")) {
          send(200, envelope(50000, "Internal Error.", 0, null));
          return;
        }
        const rows = seed.includes("nothing") ? [] : keywordsFor(seed).slice(0, task?.limit ?? 100);
        // The seed's own row is not a suggestion: it comes beside them.
        const [own, ...suggested] = rows;
        const cost = 0.012 + 0.00012 * suggested.length;
        send(
          200,
          envelope(20000, "Ok.", cost, [
            {
              se_type: "google",
              seed_keyword: seed,
              seed_keyword_data: own ? item(own) : null,
              location_code: task?.location_code ?? null,
              language_code: task?.language_code ?? null,
              total_count: suggested.length,
              items_count: suggested.length,
              items: suggested.length > 0 ? suggested.map(item) : null,
            },
          ]),
        );
      });
    });
    await new Promise<void>((resolve) => this.server!.listen(DATAFORSEO_PORT, "127.0.0.1", resolve));
  }

  async stop() {
    await new Promise<void>((resolve) => this.server?.close(() => resolve()));
  }
}
