import http from "node:http";
import { standInPort } from "./shard";

// A stand-in for VIES, the European Commission's VAT number service, for the
// browser suite: numbers whose digits start with 1 exist. config/e2e.toml
// points the dashboard at it, and scripts/e2e.sh each next server 100 ports
// further on.

export const VIES_PORT = Number(process.env.E2E_VIES_PORT ?? standInPort(12113));

export class FakeVies {
  readonly checked: string[] = [];
  private server?: http.Server;

  async start() {
    this.server = http.createServer((req, res) => {
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", () => {
        const { countryCode, vatNumber } = JSON.parse(body || "{}") as { countryCode?: string; vatNumber?: string };
        const number = `${countryCode ?? ""}${vatNumber ?? ""}`;
        this.checked.push(number);
        const valid = (vatNumber ?? "").startsWith("1");
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ valid, requestIdentifier: valid ? `WAPI${this.checked.length}` : "" }));
      });
    });
    await new Promise<void>((resolve) => this.server!.listen(VIES_PORT, "127.0.0.1", resolve));
  }

  async stop() {
    await new Promise<void>((resolve) => this.server?.close(() => resolve()));
  }
}
