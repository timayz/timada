import crypto from "node:crypto";
import http from "node:http";
import type { Page } from "@playwright/test";
import { standInPort } from "./shard";

// A stand-in for Stripe's API for the browser suite: every call goes on to
// stripe-mock (compose.yml), so the dashboard reads answers shaped exactly as
// Stripe's, and what stripe-mock forgets is kept here: the metadata and
// amount a payment was opened with, the idempotency key that opened it,
// whether the shopper confirmed it (its money held), and whether the money
// was then taken or the hold given back. config/e2e.toml points the dashboard
// at it, and scripts/e2e.sh each next server 100 ports further on.

export const STRIPE_PORT = Number(process.env.E2E_STRIPE_PORT ?? standInPort(12112));
const STRIPE_MOCK = process.env.E2E_STRIPE_MOCK_URL ?? "http://localhost:12111";

type Intent = {
  id: string;
  amount: number;
  currency: string;
  metadata: Record<string, string>;
  /// The shopper confirmed it: the money is held on their card.
  authorized: boolean;
  /// The money was taken.
  captured: boolean;
  /// The hold was given back, or the form closed.
  cancelled: boolean;
  /// `manual`: held when the shopper pays, taken by the shop.
  captureMethod: string;
};

export class FakeStripe {
  readonly intents = new Map<string, Intent>();
  private readonly byKey = new Map<string, string>();
  readonly refunds: { payment_intent: string; amount: number }[] = [];
  private server?: http.Server;

  async start() {
    this.server = http.createServer((req, res) => {
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", () => {
        this.handle(req, body)
          .then(({ status, json }) => {
            res.writeHead(status, { "content-type": "application/json" });
            res.end(JSON.stringify(json));
          })
          .catch((err) => {
            res.writeHead(500, { "content-type": "application/json" });
            res.end(JSON.stringify({ error: { message: String(err) } }));
          });
      });
    });
    await new Promise<void>((resolve) => this.server!.listen(STRIPE_PORT, "127.0.0.1", resolve));
  }

  async stop() {
    await new Promise<void>((resolve) => this.server?.close(() => resolve()));
  }

  /// The payment opened for a cart.
  intentOfCart(cartId: string): Intent | undefined {
    return [...this.intents.values()].find((intent) => intent.metadata.cart_id === cartId);
  }

  /// The last payment opened.
  lastIntent(): Intent | undefined {
    return [...this.intents.values()].at(-1);
  }

  /// The shopper confirms the payment: its money is held, to be taken.
  pay(id: string) {
    this.intents.get(id)!.authorized = true;
  }

  private async forward(req: http.IncomingMessage, body: string) {
    const response = await fetch(`${STRIPE_MOCK}${req.url}`, {
      method: req.method,
      headers: {
        authorization: req.headers.authorization ?? "Bearer sk_test_123",
        "content-type": "application/x-www-form-urlencoded",
      },
      body: req.method === "GET" ? undefined : body,
    });
    return { status: response.status, json: (await response.json()) as Record<string, unknown> };
  }

  private async handle(req: http.IncomingMessage, body: string) {
    const path = (req.url ?? "").split("?")[0];
    const answer = await this.forward(req, body);
    if (req.method === "POST" && path === "/v1/payment_intents") {
      const form = new URLSearchParams(body);
      const key = String(req.headers["idempotency-key"] ?? "");
      const known = key ? this.byKey.get(key) : undefined;
      if (!known) {
        const metadata: Record<string, string> = {};
        for (const [name, value] of form) {
          const match = /^metadata\[(.+)\]$/.exec(name);
          if (match) metadata[match[1]] = value;
        }
        const id = String(answer.json.id);
        this.intents.set(id, {
          id,
          amount: Number(form.get("amount")),
          currency: String(form.get("currency")),
          metadata,
          authorized: false,
          captured: false,
          cancelled: false,
          captureMethod: String(form.get("capture_method")),
        });
        if (key) this.byKey.set(key, id);
      }
      return { status: 200, json: this.patch(answer.json, known ?? String(answer.json.id)) };
    }
    const intent = /^\/v1\/payment_intents\/([^/]+)$/.exec(path);
    if (req.method === "GET" && intent) {
      return { status: 200, json: this.patch(answer.json, intent[1]) };
    }
    // Taking the held money, or giving the hold back: stripe-mock answers
    // any payment as it pleases, the state is kept here.
    const action = /^\/v1\/payment_intents\/([^/]+)\/(capture|cancel)$/.exec(path);
    const acted = action ? this.intents.get(action[1]) : undefined;
    if (req.method === "POST" && action && acted) {
      if (action[2] === "capture") {
        if (!acted.authorized || acted.cancelled) {
          return {
            status: 400,
            json: { error: { type: "invalid_request_error", message: "This PaymentIntent could not be captured." } },
          };
        }
        acted.captured = true;
      } else if (acted.captured) {
        return {
          status: 400,
          json: { error: { type: "invalid_request_error", message: "This PaymentIntent was captured." } },
        };
      } else {
        acted.cancelled = true;
      }
      return { status: 200, json: this.patch(answer.json, acted.id) };
    }
    if (req.method === "POST" && path === "/v1/refunds") {
      const form = new URLSearchParams(body);
      this.refunds.push({ payment_intent: String(form.get("payment_intent")), amount: Number(form.get("amount")) });
    }
    return answer;
  }

  private patch(json: Record<string, unknown>, id: string) {
    const intent = this.intents.get(id);
    if (!intent) return json;
    return {
      ...json,
      id,
      client_secret: `${id}_secret_e2e`,
      amount: intent.amount,
      amount_capturable: intent.authorized && !intent.captured && !intent.cancelled ? intent.amount : 0,
      amount_received: intent.captured ? intent.amount : 0,
      currency: intent.currency,
      metadata: intent.metadata,
      capture_method: "manual",
      // stripe-mock's charge says nothing true about this payment.
      latest_charge: null,
      cancellation_reason: intent.cancelled ? "requested_by_customer" : null,
      status: intent.captured
        ? "succeeded"
        : intent.cancelled
          ? "canceled"
          : intent.authorized
            ? "requires_capture"
            : "requires_payment_method",
    };
  }
}

/// The `Stripe-Signature` header Stripe would send with `body`.
export function stripeSignature(body: string, secret: string): string {
  const t = Math.floor(Date.now() / 1000);
  const v1 = crypto.createHmac("sha256", secret).update(`${t}.${body}`).digest("hex");
  return `t=${t},v1=${v1}`;
}

/// Stripe.js as the checkout page loads it, faked: the elements mount
/// nothing (and offer no wallet), and confirming a payment goes straight to
/// the return page as Stripe does once the shopper confirmed it, with the
/// payment's client secret in the address. What the page created the elements with is
/// kept on `window.__stripe`, the billing details it confirmed with in
/// `sessionStorage.confirmed`.
const STRIPE_JS = `
window.Stripe = (key) => {
  // The handlers an element was given, by "type:event", for a test to call.
  window.__stripe = { key, on: {} };
  return {
    elements(options) {
      window.__stripe.elements = options;
      return {
        create(type, options) {
          window.__stripe[type] = options ?? {};
          return {
            mount(target) { window.__stripe.mounted = [...(window.__stripe.mounted ?? []), target]; },
            on(event, handler) { window.__stripe.on[type + ":" + event] = handler; },
          };
        },
        update(options) { Object.assign(window.__stripe.elements, options); },
        submit: async () => ({}),
      };
    },
    async confirmPayment({ clientSecret, confirmParams }) {
      sessionStorage.setItem("confirmed", JSON.stringify(confirmParams.payment_method_data ?? null));
      const url = new URL(confirmParams.return_url);
      url.searchParams.set("payment_intent_client_secret", clientSecret);
      url.searchParams.set("redirect_status", "succeeded");
      location.assign(url.href);
      return new Promise(() => {});
    },
  };
};
`;

export async function fakeStripeJs(page: Page) {
  await page.route(/^https:\/\/js\.stripe\.com\/v3/, (route) =>
    route.fulfill({ contentType: "text/javascript", body: STRIPE_JS }),
  );
}
