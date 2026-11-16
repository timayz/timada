import http from "node:http";
import { standInPort } from "./shard";

// A stand-in for Meta's Conversions API, for the browser suite: it keeps the
// events the dashboard sends, and refuses a token that says "refused" the way
// Meta refuses a revoked one. config/e2e.toml points the dashboard at it, and
// scripts/e2e.sh each next server 100 ports further on.

export const META_PORT = Number(process.env.E2E_META_PORT ?? standInPort(12115));

/// One event as the dashboard sent it.
export type MetaEvent = {
  pixelId: string;
  token: string;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  event: any;
};

export class FakeMeta {
  readonly received: MetaEvent[] = [];
  private server?: http.Server;

  async start() {
    this.server = http.createServer((req, res) => {
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", () => {
        const pixelId = (req.url ?? "").split("/").filter(Boolean)[0] ?? "";
        const sent = JSON.parse(body || "{}") as { data?: unknown[]; access_token?: string };
        const token = sent.access_token ?? "";
        res.setHeader("content-type", "application/json");
        if (token.includes("refused")) {
          res.writeHead(400);
          res.end(
            JSON.stringify({
              error: { message: "Invalid OAuth access token.", type: "OAuthException", code: 190 },
            }),
          );
          return;
        }
        for (const event of sent.data ?? []) {
          this.received.push({ pixelId, token, event });
        }
        res.writeHead(200);
        res.end(JSON.stringify({ events_received: (sent.data ?? []).length }));
      });
    });
    await new Promise<void>((resolve) => this.server!.listen(META_PORT, "127.0.0.1", resolve));
  }

  async stop() {
    await new Promise<void>((resolve) => this.server?.close(() => resolve()));
  }
}
