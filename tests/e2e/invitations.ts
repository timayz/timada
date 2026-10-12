import { expect, type APIRequestContext, type Page } from "@playwright/test";
import { MAILDEV } from "./email";

// Invitation links are read back from MailDev like the sign-in codes.

type MailDevMessage = { to: { address: string }[]; html?: string; text?: string; date: string };

/// The invitation link in the newest message to `email`, once more than
/// `previous` messages have arrived.
export async function inviteLink(request: APIRequestContext, email: string, previous = 0): Promise<string> {
  let link = "";
  await expect(async () => {
    const response = await request.get(`${MAILDEV}/email`);
    expect(response.ok()).toBe(true);
    const messages = ((await response.json()) as MailDevMessage[])
      .filter((m) => m.to.some((t) => t.address.toLowerCase() === email.toLowerCase()))
      .sort((a, b) => new Date(a.date).getTime() - new Date(b.date).getTime());
    expect(messages.length).toBeGreaterThan(previous);
    const last = messages[messages.length - 1];
    const match = (last.html ?? last.text ?? "").match(/https?:\/\/[^\s"<]+\/invitations\/[A-Za-z0-9_-]+/);
    expect(match, "no invitation link in the latest message").not.toBeNull();
    link = match![0];
  }).toPass({ timeout: 15_000 });
  return link;
}

/// From the members page: invites `email` with the role labelled `role`.
export async function invite(page: Page, email: string, role: string) {
  await page.getByLabel("E-mail address").fill(email);
  await page.getByLabel("Invite as").selectOption({ label: role });
  await page.getByRole("button", { name: "Send invitation" }).click();
  await expect(page.getByRole("status")).toContainText(`Invitation sent to ${email.toLowerCase()}.`);
}
