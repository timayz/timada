// Passkey ceremonies. The server speaks WebAuthn Level 3 JSON, so the browser's
// parseCreationOptionsFromJSON / parseRequestOptionsFromJSON / toJSON do the
// conversions. Buttons carry their endpoints in data attributes; see
// app/auth/login.rs and app/auth/account.rs.

async function post(url, body) {
  const response = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    credentials: "same-origin",
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    throw new Error((await response.text()) || `Request failed (${response.status})`);
  }
  return response.json();
}

function nameFrom(button) {
  const input = button.dataset.nameInput && document.querySelector(button.dataset.nameInput);
  return input ? input.value : null;
}

async function register(button) {
  const options = await post(button.dataset.start, {
    mode: button.dataset.mode,
    user_id: button.dataset.userId ?? null,
    name: nameFrom(button),
  });
  const credential = await navigator.credentials.create({
    publicKey: PublicKeyCredential.parseCreationOptionsFromJSON(options.publicKey),
  });
  const done = await post(button.dataset.finish, {
    credential: credential.toJSON(),
    name: button.dataset.mode === "add_to_user" ? nameFrom(button) : "Passkey",
  });
  window.location.assign(done.location);
}

async function login(button) {
  const options = await post(button.dataset.start, {});
  const credential = await navigator.credentials.get({
    publicKey: PublicKeyCredential.parseRequestOptionsFromJSON(options.publicKey),
  });
  const done = await post(button.dataset.finish, credential.toJSON());
  window.location.assign(done.location);
}

function showError(error) {
  const box = document.querySelector("[data-auth-error]");
  if (!box) return;
  box.textContent = error instanceof Error ? error.message : String(error);
  box.classList.remove("hidden");
}

function wire(selector, action) {
  for (const button of document.querySelectorAll(selector)) {
    button.addEventListener("click", (event) => {
      event.preventDefault();
      button.disabled = true;
      action(button)
        .catch(showError)
        .finally(() => {
          button.disabled = false;
        });
    });
  }
}

const supported =
  typeof PublicKeyCredential !== "undefined" &&
  typeof PublicKeyCredential.parseCreationOptionsFromJSON === "function";

if (supported) {
  wire("[data-passkey-register]", register);
  wire("[data-passkey-login]", login);
} else {
  for (const button of document.querySelectorAll("[data-passkey-register],[data-passkey-login]")) {
    button.disabled = true;
    button.title = "Passkeys are not supported by this browser.";
  }
}
