import crypto from "node:crypto";
import http from "node:http";
import { PNG } from "./storefront";

// A stand-in for AliExpress's open platform, for the browser suite: the
// account page says yes at once, the token calls hand out fixed tokens and
// `aliexpress.ds.product.get` answers with one product in two colours whose
// stock, cost and availability a spec changes. Every call is checked against
// the app secret the way AliExpress signs (HMAC-SHA256 over the path, then the
// sorted name+value pairs, upper-case hex). config/e2e.toml points the
// dashboard at it.

export const ALIEXPRESS_PORT = Number(process.env.E2E_ALIEXPRESS_PORT ?? 12114);
export const ALIEXPRESS_URL = `http://127.0.0.1:${ALIEXPRESS_PORT}`;

export const APP_KEY = "34567";
export const APP_SECRET = "ae-e2e-s3cret";
export const PRODUCT_ID = "1005006123456789";

type Sku = { id: string; colour: string; price: string; stock: number };

export class FakeAliexpress {
  /// The calls made, by method or path, in order.
  readonly calls: string[] = [];
  /// Calls whose signature did not match.
  badSignatures = 0;
  offline = false;
  /// How long the tokens handed out last, in seconds.
  tokenLifetime = 86_400;
  readonly skus: Sku[] = [
    { id: "12000037001", colour: "Rose", price: "4.31", stock: 120 },
    { id: "12000037002", colour: "Blue", price: "5.50", stock: 0 },
  ];
  private server?: http.Server;

  async start() {
    this.server = http.createServer((req, res) => {
      req.resume();
      req.on("end", () => this.handle(req, res));
    });
    await new Promise<void>((resolve) => this.server!.listen(ALIEXPRESS_PORT, "127.0.0.1", resolve));
  }

  async stop() {
    await new Promise<void>((resolve) => this.server?.close(() => resolve()));
  }

  sku(id: string): Sku {
    return this.skus.find((sku) => sku.id === id)!;
  }

  private handle(req: http.IncomingMessage, res: http.ServerResponse) {
    const url = new URL(req.url ?? "/", ALIEXPRESS_URL);
    const json = (body: unknown) => {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    };
    if (url.pathname === "/oauth/authorize") {
      this.calls.push("authorize");
      const back = new URL(url.searchParams.get("redirect_uri") ?? "");
      back.searchParams.set("code", "e2e-code");
      back.searchParams.set("state", url.searchParams.get("state") ?? "");
      res.writeHead(302, { location: back.toString() });
      res.end();
      return;
    }
    if (url.pathname.startsWith("/img/")) {
      res.writeHead(200, { "content-type": "image/png" });
      res.end(PNG);
      return;
    }
    const path = url.pathname.startsWith("/rest") ? url.pathname.slice("/rest".length) : undefined;
    const method = path ?? url.searchParams.get("method") ?? "";
    this.calls.push(method);
    if (!this.signed(url, path)) {
      this.badSignatures += 1;
      json({ error_response: { code: "IncompleteSignature", msg: "The request signature does not conform", request_id: "e2e" } });
      return;
    }
    if (method === "/auth/token/create" || method === "/auth/token/refresh") {
      json({
        code: "0",
        access_token: "e2e-access",
        refresh_token: "e2e-refresh",
        expires_in: this.tokenLifetime,
        refresh_expires_in: 172_800,
        account: "shop@example.org",
        request_id: "e2e",
      });
      return;
    }
    if (method === "aliexpress.ds.product.get") {
      json(this.product());
      return;
    }
    json({ error_response: { code: "InvalidApiPath", msg: `unknown call ${method}`, request_id: "e2e" } });
  }

  private signed(url: URL, path: string | undefined): boolean {
    const params = [...url.searchParams.entries()].filter(([name]) => name !== "sign");
    params.sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
    const base = (path ?? "") + params.map(([name, value]) => name + value).join("");
    const expected = crypto.createHmac("sha256", APP_SECRET).update(base).digest("hex").toUpperCase();
    return url.searchParams.get("sign") === expected;
  }

  private product() {
    return {
      aliexpress_ds_product_get_response: {
        rsp_code: "200",
        rsp_msg: "Call succeeds",
        result: {
          ae_item_base_info_dto: {
            product_id: Number(PRODUCT_ID),
            subject: "Cat Tunnel Deluxe",
            detail: "<p>A <b>crinkly</b> tunnel cats hide in.</p>",
            currency_code: "EUR",
            product_status_type: this.offline ? "offline" : "onSelling",
          },
          ae_multimedia_info_dto: { image_urls: `${ALIEXPRESS_URL}/img/main1.png;${ALIEXPRESS_URL}/img/main2.png` },
          ae_item_sku_info_dtos: {
            ae_item_sku_info_d_t_o: this.skus.map((sku) => ({
              sku_id: sku.id,
              offer_sale_price: sku.price,
              sku_available_stock: sku.stock,
              sku_stock: sku.stock > 0,
              ae_sku_property_dtos: {
                ae_sku_property_d_t_o: [
                  {
                    sku_property_name: "Colour",
                    sku_property_value: sku.colour,
                    sku_image: `${ALIEXPRESS_URL}/img/${sku.colour.toLowerCase()}.png`,
                  },
                ],
              },
            })),
          },
          ae_item_properties: { ae_item_property: [{ attr_name: "Material", attr_value: "Polyester" }] },
        },
      },
    };
  }
}
