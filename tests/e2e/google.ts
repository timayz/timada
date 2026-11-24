import http from "node:http";
import { standInPort } from "./shard";

// A stand-in for Google, for the browser suite: the page where an account
// says yes (it does at once, and sends the browser back with a code), the
// address a code is exchanged and a grant renewed at, the one that names
// the account, Search Console's list of sites and, for a site, what it did
// in search: by day for the days in `searchDays`, then each day's queries
// and pages. `searchFails` makes those answer the way Google's own failure
// does. `refuse` makes the
// account say no, `withdraw()` ends every grant given so far the way an
// account does from its own settings, and `grants` leaves boxes unticked.
// config/e2e.toml points the dashboard at it, and scripts/e2e.sh each next
// server 100 ports further on.

export const GOOGLE_PORT = Number(process.env.E2E_GOOGLE_PORT ?? standInPort(12117));

/// The application config/e2e.toml gives the dashboard.
export const GOOGLE_CLIENT_ID = "timada-e2e.apps.googleusercontent.com";
export const GOOGLE_CLIENT_SECRET = "google-e2e-s3cret";

export const SEARCH_CONSOLE_SCOPE = "https://www.googleapis.com/auth/webmasters.readonly";

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

/// A question about a site's numbers, as the dashboard sent it.
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
  /// Search Console's numbers fail the way Google's own servers do.
  searchFails = false;
  /// The questions about a site's numbers.
  readonly questions: SearchQuestion[] = [];
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
      const dimension = asked.dimensions?.[0] ?? "";
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
      const row = (key: string, of: { clicks: number; impressions: number; position: number }) => ({
        keys: [key],
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
      } else {
        rows = this.searchPages.map((address, at) =>
          row(address, { clicks: 5 - at, impressions: 100 - 10 * at, position: 3 + at }),
        );
      }
      // Google leaves `rows` out when there is none.
      send(200, rows.length > 0 ? { rows, responseAggregationType: "byProperty" } : {});
      return;
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
