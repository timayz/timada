// The scenario editor's dragging, on top of a page that works without it.
//
// Every button of the editor is a submit button of one form, named `op`:
// this script sends the form without leaving the page and swaps in the
// editor the server renders back. A drag only ever presses one of those
// buttons for the merchant: a palette block dropped on a slot is
// `add:<slot>:<kind>`, a block's handle dropped on a slot is
// `move:<step>:<slot>`. Everything else (the "+" menus, Move / Move here,
// the settings) works the same with or without it.
//
// The classes it relies on are written in the Rust markup, which is what
// the stylesheet is built from: this file only sets data attributes.

const THRESHOLD = 6;
const EDGE = 48;

let root = document.querySelector("[data-scenario-editor]");
const live = document.getElementById("editor-live");
let busy = false;
let leaving = false;
let drag = null;

if (root) enhance();

function storageKey() {
  return `timada:scenario:${root.dataset.scenario}`;
}

function store() {
  try {
    if (root.hasAttribute("data-dirty")) {
      const form = new FormData(root);
      sessionStorage.setItem(
        storageKey(),
        JSON.stringify({
          base: root.dataset.base,
          definition: form.get("definition"),
          name: form.get("name"),
        }),
      );
    } else {
      sessionStorage.removeItem(storageKey());
    }
  } catch {
    // Storage refused (private mode): nothing kept, nothing lost.
  }
}

function stored() {
  try {
    const copy = JSON.parse(sessionStorage.getItem(storageKey()) || "null");
    return copy && copy.base === root.dataset.base ? copy : null;
  } catch {
    return null;
  }
}

function enhance() {
  for (const handle of root.querySelectorAll("[data-drag-handle]")) handle.hidden = false;
  root.setAttribute("data-enhanced", "");
  const banner = root.querySelector("[data-restore]");
  const copy = !root.hasAttribute("data-dirty") && stored();
  if (banner && copy && copy.definition !== new FormData(root).get("definition")) {
    banner.hidden = false;
  }
  store();
}

async function send(submitter, extra) {
  if (busy) return;
  busy = true;
  root.setAttribute("aria-busy", "true");
  const form = new FormData(root, submitter || undefined);
  for (const [key, value] of Object.entries(extra || {})) form.set(key, value);
  try {
    // URL-encoded, as a plain form posts it.
    const response = await fetch(root.action, { method: "POST", body: new URLSearchParams(form) });
    if (response.redirected) {
      leaving = true;
      try {
        sessionStorage.removeItem(storageKey());
      } catch {}
      location.assign(response.url);
      return;
    }
    const html = await response.text();
    const page = new DOMParser().parseFromString(html, "text/html");
    const next = page.querySelector("[data-scenario-editor]");
    if (!next) {
      // An error page, or the session ended: show it as the browser would.
      leaving = true;
      document.open();
      document.write(html);
      document.close();
      return;
    }
    const notices = noticesBeside(next).map((notice) => document.importNode(notice, true));
    for (const old of noticesBeside(root)) old.remove();
    swap(document.importNode(next, true));
    for (const notice of notices) root.before(notice);
  } catch {
    if (live) live.textContent = "The change could not be sent. Check the connection and try again.";
  } finally {
    busy = false;
    root?.removeAttribute("aria-busy");
  }
}

// The flash notices of a page sit next to the editor's form: those of the
// page the server sent replace the ones shown.
function noticesBeside(form) {
  return [...form.parentElement.children].filter(
    (element) => element !== form && element.matches("[role=alert], [role=status]"),
  );
}

function swap(next) {
  root.replaceWith(next);
  root = next;
  enhance();
  const focus = root.querySelector("[autofocus]");
  if (focus) focus.focus();
  if (live) live.textContent = root.dataset.announce || "";
}

document.addEventListener("submit", (event) => {
  if (event.target !== root) return;
  event.preventDefault();
  send(event.submitter);
});

// A select of the settings that changes what the others offer applies at
// once (a rule's field decides its comparisons).
document.addEventListener("change", (event) => {
  if (root && root.contains(event.target) && event.target.matches("[data-auto-apply]")) {
    send(null, { op: "apply" });
  }
});

document.addEventListener("input", (event) => {
  if (root && root.contains(event.target)) root.setAttribute("data-dirty", "");
});

document.addEventListener("click", (event) => {
  if (!root) return;
  if (event.target.closest("[data-restore-yes]")) {
    const copy = stored();
    if (copy) send(null, { op: "apply", definition: copy.definition, name: copy.name });
  } else if (event.target.closest("[data-restore-no]")) {
    try {
      sessionStorage.removeItem(storageKey());
    } catch {}
    root.querySelector("[data-restore]").hidden = true;
  }
}, true);

window.addEventListener("beforeunload", (event) => {
  if (!leaving && root && root.hasAttribute("data-dirty")) {
    store();
    event.preventDefault();
    event.returnValue = "";
  }
});

// Dragging: a palette block, or a block of the flow by its handle.

document.addEventListener("pointerdown", (event) => {
  if (!root || busy || event.button !== 0) return;
  const handle = event.target.closest("[data-drag-handle]");
  const block = event.target.closest("[data-block-kind]");
  const source = handle ? handle.closest("[data-step]") : block;
  if (!source || !root.contains(source) || (block && block.disabled)) return;
  drag = {
    source,
    kind: block ? block.dataset.blockKind : null,
    step: handle ? source.dataset.step : null,
    label: source.dataset.dragLabel || "",
    x: event.clientX,
    y: event.clientY,
    pointer: event.pointerId,
    started: false,
    target: null,
    ghost: null,
  };
});

document.addEventListener("pointermove", (event) => {
  if (!drag || event.pointerId !== drag.pointer) return;
  if (!drag.started) {
    if (Math.hypot(event.clientX - drag.x, event.clientY - drag.y) < THRESHOLD) return;
    start(event);
  }
  event.preventDefault();
  follow(event);
});

document.addEventListener("pointerup", (event) => {
  if (!drag || event.pointerId !== drag.pointer) return;
  const done = drag;
  drag = null;
  if (!done.started) return;
  swallowClickOn(done.source);
  stop(done);
  const slot = done.target;
  if (!slot) return;
  const at = slot.dataset.dropSlot;
  send(null, { op: done.step ? `move:${done.step}:${at}` : `add:${at}:${done.kind}` });
});

document.addEventListener("pointercancel", () => cancel());
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && drag && drag.started) cancel();
});

// The click a drag ends with, on the block it started from, is not a press
// of that block (a palette block would pick itself up).
function swallowClickOn(source) {
  const swallow = (event) => {
    if (source.contains(event.target)) {
      event.preventDefault();
      event.stopPropagation();
    }
  };
  document.addEventListener("click", swallow, { capture: true, once: true });
  setTimeout(() => document.removeEventListener("click", swallow, { capture: true }), 50);
}

function cancel() {
  if (!drag) return;
  const done = drag;
  drag = null;
  if (done.started) stop(done);
}

function start(event) {
  drag.started = true;
  root.setAttribute("data-dragging", "");
  drag.source.setAttribute("data-dragged", "");
  const template = root.querySelector("template[data-drag-ghost]");
  const ghost = template.content.firstElementChild.cloneNode(true);
  ghost.textContent = drag.label;
  ghost.setAttribute("aria-hidden", "true");
  document.body.append(ghost);
  drag.ghost = ghost;
  try {
    drag.source.setPointerCapture(event.pointerId);
  } catch {}
  if (live) live.textContent = `Dragging ${drag.label}. Drop it between two steps, or press Escape.`;
}

function follow(event) {
  drag.ghost.style.transform = `translate(${event.clientX + 12}px, ${event.clientY + 12}px)`;
  const under = document.elementFromPoint(event.clientX, event.clientY);
  let slot = under ? under.closest("[data-drop-slot]") : null;
  // Not into the dragged block itself.
  if (slot && (!root.contains(slot) || (drag.step && drag.source.contains(slot)))) slot = null;
  if (slot !== drag.target) {
    drag.target?.removeAttribute("data-drop-target");
    slot?.setAttribute("data-drop-target", "");
    drag.target = slot;
  }
  scrollNear(event);
}

function scrollNear(event) {
  if (event.clientY < EDGE) window.scrollBy(0, -16);
  else if (event.clientY > window.innerHeight - EDGE) window.scrollBy(0, 16);
  const canvas = root.querySelector("[data-canvas]");
  if (!canvas) return;
  const box = canvas.getBoundingClientRect();
  if (event.clientX < box.left + EDGE) canvas.scrollLeft -= 16;
  else if (event.clientX > box.right - EDGE) canvas.scrollLeft += 16;
}

function stop(done) {
  done.ghost?.remove();
  done.target?.removeAttribute("data-drop-target");
  done.source.removeAttribute("data-dragged");
  root.removeAttribute("data-dragging");
}
