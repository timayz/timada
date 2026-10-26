import crypto from "node:crypto";
import http from "node:http";
import { PNG } from "./storefront";

// A stand-in for AliExpress's open platform, for the browser suite: the
// account page says yes at once, the token calls hand out fixed tokens and
// `aliexpress.ds.product.get` answers with one product in two colours whose
// stock, cost and availability a spec changes. Orders placed with
// `aliexpress.ds.order.create` are kept, delivered by the services
// `aliexpress.logistics.buyer.freight.calculate` offers, and read back with
// `aliexpress.trade.ds.order.get` once a spec ships them. Every call is checked against
// the platform app's secret the way AliExpress signs (HMAC-SHA256 over the
// path, then the sorted name+value pairs, upper-case hex). config/e2e.toml
// gives the dashboard the same key and secret and points it at the stand-in.

export const ALIEXPRESS_PORT = Number(process.env.E2E_ALIEXPRESS_PORT ?? 12114);
export const ALIEXPRESS_URL = `http://127.0.0.1:${ALIEXPRESS_PORT}`;

export const APP_KEY = "34567";
export const APP_SECRET = "ae-e2e-s3cret";
export const PRODUCT_ID = "1005006123456789";

type Sku = { id: string; colour: string; price: string; stock: number };
type Freight = { service_name: string; cents: number; tracking: boolean };
/// An order placed: what was asked for, and its parcel once shipped.
export type PlacedOrder = {
  id: string;
  address: Record<string, string>;
  items: { product_id: number; sku_attr: string; product_count: number; logistics_service_name: string }[];
  tryToPay: boolean;
  tracking?: string;
};

export class FakeAliexpress {
  /// The calls made, by method or path, in order.
  readonly calls: string[] = [];
  /// Calls whose signature did not match.
  badSignatures = 0;
  offline = false;
  /// How long the tokens handed out last, in seconds.
  tokenLifetime = 86_400;
  /// The refresh tokens handed out, in order (each one unique, so a spec can
  /// tell its shop's renewal from another's).
  readonly refreshTokens: string[] = [];
  /// The refresh tokens renewals were asked with, in order.
  readonly renewedWith: string[] = [];
  readonly skus: Sku[] = [
    { id: "12000037001", colour: "Rose", price: "4.31", stock: 120 },
    { id: "12000037002", colour: "Blue", price: "5.50", stock: 0 },
  ];
  /// The ways AliExpress delivers, to anywhere.
  freight: Freight[] = [
    { service_name: "CAINIAO_PREMIUM", cents: 990, tracking: true },
    { service_name: "CAINIAO_STANDARD", cents: 0, tracking: true },
  ];
  /// The orders placed, in order.
  readonly orders: PlacedOrder[] = [];
  private nextOrder = 8_100_000_000_000_001;
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

  /// How an order names a combination.
  skuAttr(sku: Sku): string {
    return `14:${this.skus.indexOf(sku) + 1}#${sku.colour}`;
  }

  /// The seller sends the parcel of an order.
  ship(orderId: string, tracking: string) {
    this.orders.find((order) => order.id === orderId)!.tracking = tracking;
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
      if (method === "/auth/token/refresh") {
        this.renewedWith.push(url.searchParams.get("refresh_token") ?? "");
      }
      const refreshToken = `e2e-refresh-${crypto.randomUUID()}`;
      this.refreshTokens.push(refreshToken);
      json({
        code: "0",
        access_token: "e2e-access",
        refresh_token: refreshToken,
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
    if (method === "aliexpress.logistics.buyer.freight.calculate") {
      const query = JSON.parse(url.searchParams.get("param_aeop_freight_calculate_for_buyer_d_t_o") ?? "{}");
      if (!query.product_id || !query.sku_id || !query.country_code) {
        json({ aliexpress_logistics_buyer_freight_calculate_response: { result: { success: false, error_desc: "missing parameters" } } });
        return;
      }
      json({
        aliexpress_logistics_buyer_freight_calculate_response: {
          result: {
            success: true,
            aeop_freight_calculate_result_for_buyer_d_t_o_list: {
              aeop_freight_calculate_result_for_buyer_dto: this.freight.map((option) => ({
                service_name: option.service_name,
                freight: { cent: option.cents, amount: (option.cents / 100).toFixed(2), currency_code: "EUR" },
                tracking_available: String(option.tracking),
                estimated_delivery_time: "15-25",
              })),
            },
          },
        },
      });
      return;
    }
    if (method === "aliexpress.ds.order.create") {
      const request = JSON.parse(url.searchParams.get("param_place_order_request4_open_api_d_t_o") ?? "{}");
      const extend = JSON.parse(url.searchParams.get("ds_extend_request") ?? "{}");
      const items: PlacedOrder["items"] = request.product_items ?? [];
      const known = items.every(
        (item) =>
          String(item.product_id) === PRODUCT_ID &&
          this.skus.some((sku) => this.skuAttr(sku) === item.sku_attr) &&
          this.freight.some((option) => option.service_name === item.logistics_service_name),
      );
      const address = request.logistics_address ?? {};
      if (!known || items.length === 0) {
        json({ aliexpress_trade_buy_placeorder_response: { result: { is_success: false, error_code: "ERROR_WHEN_BUILD_FOR_PLACE_ORDER", error_msg: "unknown item" } } });
        return;
      }
      if (!address.address || !address.country || !address.full_name) {
        json({ aliexpress_trade_buy_placeorder_response: { result: { is_success: false, error_code: "B_DROPSHIPPER_DELIVERY_ADDRESS_VALIDATE_FAIL", error_msg: "address" } } });
        return;
      }
      const id = String(this.nextOrder++);
      this.orders.push({ id, address, items, tryToPay: extend.payment?.try_to_pay === "true" });
      json({ aliexpress_trade_buy_placeorder_response: { result: { is_success: true, order_list: { number: [Number(id)] } } } });
      return;
    }
    if (method === "aliexpress.trade.ds.order.get") {
      const id = url.searchParams.get("order_id") ?? "";
      const order = this.orders.find((placed) => placed.id === id);
      if (!order) {
        json({ aliexpress_trade_ds_order_get_response: { rsp_code: "404", rsp_msg: "order not found" } });
        return;
      }
      json({
        aliexpress_trade_ds_order_get_response: {
          rsp_code: "200",
          rsp_msg: "Call succeeds",
          result: {
            order_status: order.tracking ? "WAIT_BUYER_ACCEPT_GOODS" : order.tryToPay ? "WAIT_SELLER_SEND_GOODS" : "PLACE_ORDER_SUCCESS",
            logistics_status: order.tracking ? "SELLER_SEND_GOODS" : "WAIT_SELLER_SEND_GOODS",
            logistics_info_list: order.tracking
              ? { ae_order_logistics_info: [{ logistics_no: order.tracking, logistics_service: "CAINIAO_STANDARD" }] }
              : {},
          },
        },
      });
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
              sku_attr: this.skuAttr(sku),
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
