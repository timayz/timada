import http from "node:http";
import { standInPort } from "./shard";

// A stand-in for the model that drafts texts (Anthropic's Messages API), for
// the browser suite: it writes four texts from the search it is asked to
// write for. A search that says "refused" is declined the way the model's
// safeguards decline, one that says "unavailable" fails the way an
// overloaded API does, and one that says "invente" gets a draft stating a
// length and a material nothing gave it.
// config/e2e.toml points the dashboard at it, and scripts/e2e.sh each next
// server 100 ports further on.

export const WRITER_PORT = Number(process.env.E2E_WRITER_PORT ?? standInPort(12118));

/// The key config/e2e.toml gives the dashboard.
export const WRITER_KEY = "writer-e2e-s3cret";

/// What the stand-in counts a call at: 1,500 tokens in and 900 out, which
/// config/e2e.toml prices at 0.024 USD.
export const WRITER_USAGE = { input_tokens: 1500, output_tokens: 900 };

/// One call as the dashboard sent it.
export type WriterCall = {
  /// The search the texts were asked for.
  keyword: string;
  product: boolean;
  /// The request as the model read it.
  said: string;
  model: string;
};

/// The texts the stand-in writes for `keyword`: what a spec expects to read
/// on the proposal. A category's name is not the writer's to change.
export function textsFor(keyword: string, product: boolean) {
  const invents = keyword.includes("invente");
  return {
    name: product ? `${keyword} rédigé` : "",
    description: invents
      ? `Un modèle de 30 cm en cuir pour ${keyword}.`
      : `Tout ce qu'il faut savoir sur ${keyword}.`,
    meta_title: `${keyword} | boutique`,
    meta_description: `Découvrez ${keyword} et ce qui le distingue.`,
  };
}

export class FakeWriter {
  /// Every call the stand-in answered, in order.
  calls: WriterCall[] = [];
  private server?: http.Server;

  async start() {
    this.server = http.createServer((req, res) => {
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", () => {
        const send = (status: number, payload: unknown) => {
          res.writeHead(status, { "content-type": "application/json" });
          res.end(JSON.stringify(payload));
        };
        const failure = (status: number, type: string, message: string) =>
          send(status, { type: "error", error: { type, message } });
        if (req.method !== "POST" || req.url !== "/v1/messages") {
          failure(404, "not_found_error", "Not found");
          return;
        }
        if (req.headers["x-api-key"] !== WRITER_KEY) {
          failure(401, "authentication_error", "invalid x-api-key");
          return;
        }
        if (!req.headers["anthropic-version"]) {
          failure(400, "invalid_request_error", "anthropic-version: header is required");
          return;
        }
        const request = JSON.parse(body || "{}") as {
          model?: string;
          messages?: { role: string; content: string }[];
          output_config?: { format?: { type?: string } };
        };
        if (request.output_config?.format?.type !== "json_schema") {
          failure(400, "invalid_request_error", "output_config.format: a JSON schema is expected");
          return;
        }
        const said = request.messages?.[0]?.content ?? "";
        const keyword = said.match(/^Search to write for: (.*)$/m)?.[1] ?? "";
        const product = said.startsWith("Write the texts of this product.");
        this.calls.push({ keyword, product, said, model: request.model ?? "" });
        if (keyword.includes("unavailable")) {
          failure(529, "overloaded_error", "Overloaded");
          return;
        }
        const message = {
          id: `msg_${this.calls.length}`,
          type: "message",
          role: "assistant",
          model: request.model,
          usage: WRITER_USAGE,
        };
        if (keyword.includes("refused")) {
          send(200, { ...message, content: [], stop_reason: "refusal", stop_details: { type: "refusal", category: null } });
          return;
        }
        send(200, {
          ...message,
          // The model thinks first; its thinking comes back empty.
          content: [
            { type: "thinking", thinking: "", signature: "sig" },
            { type: "text", text: JSON.stringify(textsFor(keyword, product)) },
          ],
          stop_reason: "end_turn",
        });
      });
    });
    await new Promise<void>((resolve) => this.server!.listen(WRITER_PORT, "127.0.0.1", resolve));
  }

  async stop() {
    // The dashboard keeps its connection open between calls.
    this.server?.closeAllConnections();
    await new Promise<void>((resolve) => this.server?.close(() => resolve()));
  }
}
