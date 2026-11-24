import http from "node:http";
import { standInPort } from "./shard";

// A stand-in for Google, for the browser suite: the page where an account
// says yes (it does at once, and sends the browser back with a code), the
// address a code is exchanged and a grant renewed at, the one that names
// the account, Search Console's list of sites and, for a site, what it did
// in search: by day for the days in `searchDays`, then each day's queries
// and pages, and which page showed for which query (`searchPageQueries`).
// `searchFails` makes those answer the way Google's own failure
// does. Google Ads answers too: the accounts the grant reaches
// (`adsAccounts`) and, for one, its four reports over the days in
// `adsDays` (`ADS_*` say what each day holds), until `adsFails`. Merchant
// Center lists its accounts (`merchantAccounts`), an account's products
// (`merchantProducts`) and what is held against it (`merchantIssues`);
// `merchantUnregistered` answers the way a Cloud project Google does not
// know for Merchant Center is answered. `refuse` makes the
// account say no, `withdraw()` ends every grant given so far the way an
// account does from its own settings, and `grants` leaves boxes unticked.
// config/e2e.toml points the dashboard at it, and scripts/e2e.sh each next
// server 100 ports further on.

export const GOOGLE_PORT = Number(process.env.E2E_GOOGLE_PORT ?? standInPort(12117));

/// The application config/e2e.toml gives the dashboard.
export const GOOGLE_CLIENT_ID = "timada-e2e.apps.googleusercontent.com";
export const GOOGLE_CLIENT_SECRET = "google-e2e-s3cret";

export const SEARCH_CONSOLE_SCOPE = "https://www.googleapis.com/auth/webmasters.readonly";
export const ADS_SCOPE = "https://www.googleapis.com/auth/adwords";
export const MERCHANT_SCOPE = "https://www.googleapis.com/auth/content";

/// A product as Merchant Center has it: where it stands for Shopping ads
/// and for the free listings (`approved`, `pending` or `disapproved`), and
/// what Google holds against it.
export type MerchantProduct = {
  offerId: string;
  ads?: "approved" | "pending" | "disapproved";
  listings?: "approved" | "pending" | "disapproved";
  issues?: { code: string; attribute?: string; description: string; detail?: string }[];
};

/// A Google Ads account the grant reaches; `manager` is the manager account
/// it is reached through.
export type AdsAccount = { id: string; name: string; currency: string; timeZone: string; manager?: string };

/// What the stand-in says an account's ads did on each day they were shown:
/// one campaign on two kinds of device, its two ad groups, and two searches.
/// Money is in the account's currency.
export const ADS_CAMPAIGN = { id: "11", name: "Shopping" };
export const ADS_DEVICES = [
  { device: "MOBILE", cost: 9, impressions: 1500, clicks: 30, conversions: 1, value: 20 },
  { device: "DESKTOP", cost: 3, impressions: 500, clicks: 10, conversions: 1, value: 20 },
];
export const ADS_GROUPS = [
  { id: "21", name: "Arbres à chat", cost: 8, impressions: 1200, clicks: 25, conversions: 2, value: 40 },
  { id: "22", name: "Griffoirs", cost: 4, impressions: 800, clicks: 15, conversions: 0, value: 0 },
];
export const ADS_TERMS = [
  { term: "arbre à chat", group: "21", cost: 6, impressions: 900, clicks: 20, conversions: 2, value: 40 },
  { term: "griffoir carton", group: "22", cost: 4, impressions: 800, clicks: 15, conversions: 0, value: 0 },
];

/// A day `back` days before today (UTC), as Search Console writes it.
export function searchDay(back: number): string {
  return new Date(Date.now() - back * 86_400_000).toISOString().slice(0, 10);
}

/// What the stand-in says a site did on each day it has finished: its
/// numbers, its queries and its pages, the same every day.
export const SEARCH_DAY = { clicks: 10, impressions: 200, position: 8.25 };
export const SEARCH_QUERIES = [
  { key: "arbre à chat", clicks: 6, impressions: 100, position: 4 },
  { key: "griffoir carton", clicks: 2, impressions: 50, position: 12.5 },
];

/// A query a page showed for on a day.
export type PageQuery = { page: string; query: string; clicks: number; impressions: number; position: number };

/// A question about a site's numbers, as the dashboard sent it; `dimension`
/// is what it groups by, `page+query` for both.
export type SearchQuestion = { site: string; first: string; last: string; dimension: string };

/// A site as Search Console lists it.
export type GoogleSite = { siteUrl: string; permissionLevel: string };

export class FakeGoogle {
  /// The account that says yes.
  account = "merchant@example.com";
  /// The sites it sees in Search Console.
  sites: GoogleSite[] = [
    { siteUrl: "sc-domain:shop.example", permissionLevel: "siteOwner" },
    { siteUrl: "https://blog.shop.example/", permissionLevel: "siteFullUser" },
    { siteUrl: "https://unproved.example/", permissionLevel: "siteUnverifiedUser" },
  ];
  /// The account says no on Google's page.
  refuse = false;
  /// What the account grants of what it was asked; everything when unset.
  grants?: (asked: string[]) => string[];
  /// The scopes each visit to the page asked for.
  readonly asked: string[][] = [];
  /// What was sent with each visit, by name.
  readonly visits: Record<string, string>[] = [];
  /// The days Google has finished counting for every site.
  searchDays: string[] = [searchDay(4), searchDay(3), searchDay(2)];
  /// The addresses found, the most clicked first; the origin is the site's.
  searchPages: string[] = ["https://shop.example/", "https://shop.example/panier"];
  /// Which page showed for which query on a day; none unless a test says.
  searchPageQueries: (day: string) => PageQuery[] = () => [];
  /// Search Console's numbers fail the way Google's own servers do.
  searchFails = false;
  /// The questions about a site's numbers.
  readonly questions: SearchQuestion[] = [];
  /// The Google Ads accounts the grant reaches; none unless a test says.
  adsAccounts: AdsAccount[] = [];
  /// The days the accounts' ads were shown on (days of the account).
  adsDays: string[] = [];
  /// The feed's items shown, one in each ad group of `ADS_GROUPS`.
  adsItems: string[] = [];
  /// Google Ads fails the way Google's own servers do.
  adsFails = false;
  /// The reports asked of Google Ads: which, of which account, over which days.
  readonly adsQuestions: { customer: string; report: string; first: string; last: string }[] = [];
  /// The Merchant Center accounts the grant reaches; none unless a test says.
  merchantAccounts: { accountId: string; accountName: string }[] = [];
  /// The products of every account, as Merchant Center has them.
  merchantProducts: MerchantProduct[] = [];
  /// What Google holds against the account itself.
  merchantIssues: { title: string; severity: string; detail: string; documentationUri: string }[] = [];
  /// The Cloud project is not registered with a Merchant Center account.
  merchantUnregistered = false;
  /// How many times the products of an account were listed.
  merchantReads = 0;
  /// How many times a grant was renewed.
  renewals = 0;
  private readonly codes = new Map<string, string[]>();
  /// By refresh token, the scopes it covers.
  private readonly grantsGiven = new Map<string, string[]>();
  private readonly accesses = new Set<string>();
  private issued = 0;
  private server?: http.Server;

  /// Ends every grant given so far, and the accesses made from them.
  withdraw() {
    this.grantsGiven.clear();
    this.accesses.clear();
  }

  async start() {
    this.server = http.createServer((req, res) => {
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", () => this.handle(req, res, body));
    });
    await new Promise<void>((resolve) => this.server!.listen(GOOGLE_PORT, "127.0.0.1", resolve));
  }

  private access(): string {
    const access = `e2e-access-${++this.issued}`;
    this.accesses.add(access);
    return access;
  }

  private handle(req: http.IncomingMessage, res: http.ServerResponse, body: string) {
    const url = new URL(req.url ?? "/", `http://127.0.0.1:${GOOGLE_PORT}`);
    const send = (status: number, answer: unknown) => {
      res.writeHead(status, { "content-type": "application/json" });
      res.end(JSON.stringify(answer));
    };
    if (req.method === "GET" && url.pathname === "/o/oauth2/v2/auth") {
      const asked = (url.searchParams.get("scope") ?? "").split(" ").filter(Boolean);
      this.asked.push(asked);
      this.visits.push(Object.fromEntries(url.searchParams));
      const back = new URL(url.searchParams.get("redirect_uri") ?? "");
      back.searchParams.set("state", url.searchParams.get("state") ?? "");
      if (this.refuse || url.searchParams.get("client_id") !== GOOGLE_CLIENT_ID) {
        back.searchParams.set("error", "access_denied");
      } else {
        const code = `e2e-code-${++this.issued}`;
        this.codes.set(code, this.grants ? this.grants(asked) : asked);
        back.searchParams.set("code", code);
      }
      res.writeHead(302, { location: back.toString() });
      res.end();
      return;
    }
    if (req.method === "POST" && url.pathname === "/token") {
      const form = new URLSearchParams(body);
      if (form.get("client_id") !== GOOGLE_CLIENT_ID || form.get("client_secret") !== GOOGLE_CLIENT_SECRET) {
        send(401, { error: "invalid_client", error_description: "The OAuth client was not found." });
        return;
      }
      if (form.get("grant_type") === "authorization_code") {
        const scopes = this.codes.get(form.get("code") ?? "");
        this.codes.delete(form.get("code") ?? "");
        if (!scopes) {
          send(400, { error: "invalid_grant", error_description: "Bad Request" });
          return;
        }
        const refresh = `e2e-refresh-${++this.issued}`;
        this.grantsGiven.set(refresh, scopes);
        send(200, {
          access_token: this.access(),
          expires_in: 3599,
          refresh_token: refresh,
          scope: scopes.join(" "),
          token_type: "Bearer",
        });
        return;
      }
      const scopes = this.grantsGiven.get(form.get("refresh_token") ?? "");
      if (form.get("grant_type") !== "refresh_token" || !scopes) {
        send(400, { error: "invalid_grant", error_description: "Token has been expired or revoked." });
        return;
      }
      this.renewals += 1;
      send(200, { access_token: this.access(), expires_in: 3599, scope: scopes.join(" "), token_type: "Bearer" });
      return;
    }
    const bearer = (req.headers.authorization ?? "").replace(/^Bearer /, "");
    if (!this.accesses.has(bearer)) {
      send(401, { error: { code: 401, message: "Invalid Credentials", status: "UNAUTHENTICATED" } });
      return;
    }
    if (req.method === "GET" && url.pathname === "/v1/userinfo") {
      send(200, { sub: "e2e", email: this.account, email_verified: true });
      return;
    }
    if (req.method === "GET" && url.pathname === "/webmasters/v3/sites") {
      send(200, this.sites.length > 0 ? { siteEntry: this.sites } : {});
      return;
    }
    const analytics = /^\/webmasters\/v3\/sites\/(.+)\/searchAnalytics\/query$/.exec(url.pathname);
    if (req.method === "POST" && analytics) {
      const asked = JSON.parse(body || "{}") as { startDate: string; endDate: string; dimensions: string[] };
      const dimension = (asked.dimensions ?? []).join("+");
      this.questions.push({
        site: decodeURIComponent(analytics[1]),
        first: asked.startDate,
        last: asked.endDate,
        dimension,
      });
      if (this.searchFails) {
        send(500, { error: { code: 500, message: "Backend Error", status: "INTERNAL" } });
        return;
      }
      const row = (key: string | string[], of: { clicks: number; impressions: number; position: number }) => ({
        keys: [key].flat(),
        clicks: of.clicks,
        impressions: of.impressions,
        ctr: of.clicks / of.impressions,
        position: of.position,
      });
      let rows: ReturnType<typeof row>[];
      if (dimension === "date") {
        rows = this.searchDays
          .filter((day) => day >= asked.startDate && day <= asked.endDate)
          .sort()
          .map((day) => row(day, SEARCH_DAY));
      } else if (dimension === "query") {
        rows = SEARCH_QUERIES.map((query) => row(query.key, query));
      } else if (dimension === "page+query") {
        rows = this.searchPageQueries(asked.startDate).map((pair) => row([pair.page, pair.query], pair));
      } else {
        rows = this.searchPages.map((address, at) =>
          row(address, { clicks: 5 - at, impressions: 100 - 10 * at, position: 3 + at }),
        );
      }
      // Google leaves `rows` out when there is none.
      send(200, rows.length > 0 ? { rows, responseAggregationType: "byProperty" } : {});
      return;
    }
    if (req.method === "GET" && url.pathname === "/ads/v26/customers:listAccessibleCustomers") {
      // The accounts reached by themselves, and the managers of the others.
      const reached = new Set(this.adsAccounts.map((account) => account.manager ?? account.id));
      send(200, reached.size > 0 ? { resourceNames: [...reached].map((id) => `customers/${id}`) } : {});
      return;
    }
    const ads = /^\/ads\/v26\/customers\/(\d+)\/googleAds:searchStream$/.exec(url.pathname);
    if (req.method === "POST" && ads) {
      const query = (JSON.parse(body || "{}") as { query: string }).query;
      const report = / FROM (\w+)/.exec(query)?.[1] ?? "";
      if (report === "customer_client") {
        const clients = this.adsAccounts.filter((account) => account.manager === ads[1]);
        const own = this.adsAccounts.find((account) => account.id === ads[1]);
        const row = (account: AdsAccount, level: number) => ({
          customerClient: {
            id: account.id,
            level: `${level}`,
            manager: false,
            descriptiveName: account.name,
            currencyCode: account.currency,
            timeZone: account.timeZone,
          },
        });
        const results = own
          ? [row(own, 0)]
          : [{ customerClient: { id: ads[1], level: "0", manager: true } }, ...clients.map((c) => row(c, 1))];
        send(200, [{ results }]);
        return;
      }
      const [, first, last] = /BETWEEN '([\d-]+)' AND '([\d-]+)'/.exec(query) ?? [];
      this.adsQuestions.push({ customer: ads[1], report, first, last });
      if (this.adsFails) {
        send(500, [{ error: { code: 500, message: "Internal error encountered.", status: "INTERNAL" } }]);
        return;
      }
      // Counts as text and decimals as numbers, the way Google writes them.
      const metrics = (of: { cost: number; impressions: number; clicks: number; conversions: number; value: number }) => ({
        costMicros: `${of.cost * 1_000_000}`,
        impressions: `${of.impressions}`,
        clicks: `${of.clicks}`,
        conversions: of.conversions,
        conversionsValue: of.value,
      });
      const currency = this.adsAccounts.find((account) => account.id === ads[1])?.currency ?? "EUR";
      const campaign = { id: ADS_CAMPAIGN.id, name: ADS_CAMPAIGN.name };
      const results = this.adsDays
        .filter((day) => day >= first && day <= last)
        .flatMap((day) => {
          if (report === "campaign") {
            return ADS_DEVICES.map((of) => ({
              campaign,
              customer: { currencyCode: currency },
              segments: { date: day, device: of.device },
              metrics: metrics(of),
            }));
          }
          if (report === "ad_group") {
            return ADS_GROUPS.map((of) => ({
              campaign,
              adGroup: { id: of.id, name: of.name },
              segments: { date: day },
              metrics: metrics(of),
            }));
          }
          if (report === "shopping_performance_view") {
            return this.adsItems.slice(0, ADS_GROUPS.length).map((item, at) => ({
              campaign,
              adGroup: { id: ADS_GROUPS[at].id },
              segments: { date: day, productItemId: item },
              metrics: metrics(ADS_GROUPS[at]),
            }));
          }
          return ADS_TERMS.map((of) => ({
            searchTermView: { searchTerm: of.term },
            campaign,
            adGroup: { id: of.group },
            segments: { date: day },
            metrics: metrics(of),
          }));
        });
      send(200, [{ results }]);
      return;
    }
    if (req.method === "GET" && url.pathname.startsWith("/merchant/")) {
      if (this.merchantUnregistered) {
        send(401, {
          error: {
            code: 401,
            message: "GCP project with id timada-e2e and number 12345 is not registered with the merchant account.",
            status: "UNAUTHENTICATED",
          },
        });
        return;
      }
      if (url.pathname === "/merchant/accounts/v1/accounts") {
        send(200, this.merchantAccounts.length > 0 ? { accounts: this.merchantAccounts } : {});
        return;
      }
      if (/^\/merchant\/accounts\/v1\/accounts\/\d+\/issues$/.test(url.pathname)) {
        send(200, this.merchantIssues.length > 0 ? { accountIssues: this.merchantIssues } : {});
        return;
      }
      if (/^\/merchant\/products\/v1\/accounts\/\d+\/products$/.test(url.pathname)) {
        this.merchantReads += 1;
        const place = (context: string, standing?: string) =>
          standing ? [{ reportingContext: context, [`${standing}Countries`]: ["FR"] }] : [];
        const products = this.merchantProducts.map((product) => ({
          name: `accounts/1/products/fr~FR~${product.offerId}`,
          offerId: product.offerId,
          productStatus: {
            destinationStatuses: [
              ...place("SHOPPING_ADS", product.ads),
              ...place("FREE_LISTINGS", product.listings),
            ],
            itemLevelIssues: (product.issues ?? []).map((issue) => ({
              code: issue.code,
              severity: "DISAPPROVED",
              resolution: "merchant_action",
              attribute: issue.attribute ?? "",
              reportingContext: "SHOPPING_ADS",
              description: issue.description,
              detail: issue.detail ?? "",
              documentation: "https://support.google.com/merchants/answer/6324350",
            })),
          },
        }));
        send(200, products.length > 0 ? { products } : {});
        return;
      }
    }
    send(404, { error: { code: 404, message: "Not Found" } });
  }

  async stop() {
    const closed = new Promise<void>((resolve) => this.server?.close(() => resolve()));
    // The dashboard's sweep and the browser keep a connection open, and a
    // connection in use never counts as closed.
    this.server?.closeAllConnections();
    await closed;
  }
}
