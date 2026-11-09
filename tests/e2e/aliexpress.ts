import type { APIRequestContext } from "@playwright/test";
import crypto from "node:crypto";
import http from "node:http";
import { PNG } from "./storefront";

// A stand-in for AliExpress's open platform, for the browser suite: the
// account page says yes at once, the token calls hand out fixed tokens and
// `aliexpress.ds.product.get` answers with one product in two colours whose
// stock, cost and availability a spec changes. Orders placed with
// `aliexpress.ds.order.create` are kept, delivered by the services
// `aliexpress.ds.freight.query` offers, and read back with
// `aliexpress.trade.ds.order.get` once a spec ships them, or pushed to the
// dashboard's webhook as AliExpress would. Every call is checked against
// the platform app's secret the way AliExpress signs (HMAC-SHA256 over the
// path, then the sorted name+value pairs, upper-case hex). config/e2e.toml
// gives the dashboard the same key and secret and points it at the stand-in.

export const ALIEXPRESS_PORT = Number(process.env.E2E_ALIEXPRESS_PORT ?? 12114);
export const ALIEXPRESS_URL = `http://127.0.0.1:${ALIEXPRESS_PORT}`;

export const APP_KEY = "34567";
export const APP_SECRET = "ae-e2e-s3cret";
export const PRODUCT_ID = "1005006123456789";

type Sku = { id: string; colour: string; price: string; stock: number; shipsFrom?: string };
type Freight = { service_name: string; cents: number; tracking: boolean };
/// The number of the next order placed, whichever stand-in takes it:
/// AliExpress never gives one number twice, and the platform finds a push's
/// shop by it. Started from the clock, so a worker started again does not
/// hand out the numbers of the one before.
let nextOrder = 8_100_000_000_000_001 + (Date.now() % 100_000_000) * 10;

/// An order placed: what was asked for, and its parcel once shipped.
export type PlacedOrder = {
  id: string;
  address: Record<string, string>;
  items: { product_id: number; sku_attr: string; product_count: number; logistics_service_name: string }[];
  tryToPay: boolean;
  tracking?: string;
  /// Closed by AliExpress without shipping.
  closed?: boolean;
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
  /// How the next orders placed are answered, in turn: `lost` takes the
  /// order and answers with no number (AliExpress may have it), `refused`
  /// refuses it outright. Taken as usual once none is left.
  readonly placeAnswers: ("lost" | "refused")[] = [];
  private server?: http.Server;

  async start() {
    this.server = http.createServer((req, res) => {
      req.resume();
      req.on("end", () => this.handle(req, res));
    });
    await new Promise<void>((resolve) => this.server!.listen(ALIEXPRESS_PORT, "127.0.0.1", resolve));
  }

  async stop() {
    const closed = new Promise<void>((resolve) => this.server?.close(() => resolve()));
    // The sweeps of other specs' shops keep a pooled connection busy, and a
    // connection in use never counts as closed.
    this.server?.closeAllConnections();
    await closed;
  }

  sku(id: string): Sku {
    return this.skus.find((sku) => sku.id === id)!;
  }

  /// How an order names a combination.
  skuAttr(sku: Sku): string {
    const attr = `14:${this.skus.indexOf(sku) + 1}#${sku.colour}`;
    return sku.shipsFrom ? `${attr};200007763:${this.skus.indexOf(sku) + 1}#${sku.shipsFrom}` : attr;
  }

  /// The seller sends the parcel of an order.
  ship(orderId: string, tracking: string) {
    this.orders.find((order) => order.id === orderId)!.tracking = tracking;
  }

  /// AliExpress closes an order without shipping it (the seller cancelled,
  /// or it was never paid).
  close(orderId: string) {
    this.orders.find((order) => order.id === orderId)!.closed = true;
  }

  /// AliExpress pushes an order's new status to the platform's webhook at
  /// `url` (its message push: the body signed with the app's key and
  /// secret in `Authorization`); the response.
  push(request: APIRequestContext, url: string, orderId: string, status: string, options: { signature?: string; body?: string } = {}) {
    const body =
      options.body ??
      JSON.stringify({
        data: { buyerId: 1861703697, orderId: Number(orderId), orderStatus: status },
        message_type: 53,
        seller_id: "1234",
        site: "ae_global",
        timestamp: Math.floor(Date.now() / 1000),
      });
    return request.post(url, {
      data: body,
      headers: { "content-type": "application/json", authorization: options.signature ?? pushSignature(body) },
    });
  }

  /// How many times an order was read (`aliexpress.trade.ds.order.get`).
  orderReads(): number {
    return this.calls.filter((call) => call === "aliexpress.trade.ds.order.get").length;
  }

  /// How many times a parcel was asked about (`aliexpress.logistics.ds.trackinginfo.query`).
  parcelReads(): number {
    return this.calls.filter((call) => call === "aliexpress.logistics.ds.trackinginfo.query").length;
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
    if (method === "aliexpress.ds.freight.query") {
      const query = JSON.parse(url.searchParams.get("queryDeliveryReq") ?? "{}");
      if (!query.productId || !query.selectedSkuId || !query.shipToCountry || !query.quantity || !query.currency || !query.language || !query.locale) {
        json({ aliexpress_ds_freight_query_response: { result: { success: false, code: 500, msg: "missing parameters" } } });
        return;
      }
      json({
        aliexpress_ds_freight_query_response: {
          result: {
            success: true,
            code: 200,
            delivery_options: {
              delivery_option_d_t_o: this.freight.map((option) => ({
                code: option.service_name,
                free_shipping: option.cents === 0,
                shipping_fee_cent: String(option.cents),
                shipping_fee_currency: "EUR",
                tracking: option.tracking,
                ship_from_country: "CN",
                min_delivery_days: 7,
                max_delivery_days: 12,
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
      if (!address.address || !address.country || !address.full_name || !address.province || !address.mobile_no) {
        json({ aliexpress_trade_buy_placeorder_response: { result: { is_success: false, error_code: "B_DROPSHIPPER_DELIVERY_ADDRESS_VALIDATE_FAIL", error_msg: "address" } } });
        return;
      }
      const answer = this.placeAnswers.shift();
      if (answer === "refused") {
        json({ aliexpress_trade_buy_placeorder_response: { result: { is_success: false, error_code: "B_DROPSHIPPER_DELIVERY_ADDRESS_VALIDATE_FAIL", error_msg: "address" } } });
        return;
      }
      const id = String(nextOrder++);
      this.orders.push({ id, address, items, tryToPay: extend.payment?.try_to_pay === "true" });
      if (answer === "lost") {
        json({ aliexpress_trade_buy_placeorder_response: { result: { is_success: true, order_list: [] } } });
        return;
      }
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
            order_status: order.tracking
              ? "WAIT_BUYER_ACCEPT_GOODS"
              : order.closed
                ? "FINISH"
                : order.tryToPay
                  ? "WAIT_SELLER_SEND_GOODS"
                  : "PLACE_ORDER_SUCCESS",
            logistics_status: order.tracking ? "SELLER_SEND_GOODS" : order.closed ? "NO_LOGISTICS" : "WAIT_SELLER_SEND_GOODS",
            logistics_info_list: order.tracking
              ? { ae_order_logistics_info: [{ logistics_no: order.tracking, logistics_service: "CAINIAO_STANDARD" }] }
              : {},
          },
        },
      });
      return;
    }
    if (method === "aliexpress.logistics.ds.trackinginfo.query") {
      // Every parcel shipped has left China; the steps are the same for all.
      const number = url.searchParams.get("logistics_no") ?? "";
      const shipped = this.orders.some((placed) => placed.tracking === number);
      json({
        aliexpress_logistics_ds_trackinginfo_query_response: shipped
          ? {
              result_success: true,
              details: {
                details: [
                  { event_desc: "Shipment accepted by carrier", address: "Shenzhen", event_date: "2026-11-08 10:12:00" },
                  { event_desc: "Departed from origin country", address: "Shenzhen", event_date: "2026-11-09 08:00:00" },
                ],
              },
            }
          : { result_success: false, result_error: "NO_TRACKING", error_desc: "no such parcel" },
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
            // The seller's listing currency; the combinations are quoted
            // in the market's.
            currency_code: "CNY",
            product_status_type: this.offline ? "offline" : "onSelling",
          },
          ae_multimedia_info_dto: { image_urls: `${ALIEXPRESS_URL}/img/main1.png;${ALIEXPRESS_URL}/img/main2.png` },
          ae_item_sku_info_dtos: {
            ae_item_sku_info_d_t_o: this.skus.map((sku) => ({
              sku_id: sku.id,
              sku_attr: this.skuAttr(sku),
              offer_sale_price: sku.price,
              currency_code: "EUR",
              sku_available_stock: sku.stock,
              sku_stock: sku.stock > 0,
              ae_sku_property_dtos: {
                ae_sku_property_d_t_o: [
                  {
                    sku_property_name: "Colour",
                    sku_property_value: sku.colour,
                    sku_image: `${ALIEXPRESS_URL}/img/${sku.colour.toLowerCase()}.png`,
                  },
                  ...(sku.shipsFrom ? [{ sku_property_name: "Ships From", sku_property_value: sku.shipsFrom }] : []),
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

/// What AliExpress puts in `Authorization` for a pushed `body`: HMAC-SHA256
/// of the app key followed by the body, keyed by the secret, in lower hex.
export function pushSignature(body: string, appKey = APP_KEY, appSecret = APP_SECRET): string {
  return crypto.createHmac("sha256", appSecret).update(appKey + body).digest("hex");
}
