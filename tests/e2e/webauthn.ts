import type { CDPSession, Page } from "@playwright/test";

// A CTAP2 platform authenticator with resident keys and user verification,
// as a phone or laptop with biometrics would behave. Chrome's WebAuthn
// virtual authenticator answers every ceremony without a prompt.

export type Authenticator = { cdp: CDPSession; id: string };

/// Adds a virtual authenticator to the page's browser, on `cdp` when the
/// test already holds a session.
export async function virtualAuthenticator(
  page: Page,
  cdp?: CDPSession,
): Promise<Authenticator> {
  cdp ??= await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable");
  const { authenticatorId } = await cdp.send(
    "WebAuthn.addVirtualAuthenticator",
    {
      options: {
        protocol: "ctap2",
        transport: "internal",
        hasResidentKey: true,
        hasUserVerification: true,
        isUserVerified: true,
        automaticPresenceSimulation: true,
      },
    },
  );
  return { cdp, id: authenticatorId };
}
