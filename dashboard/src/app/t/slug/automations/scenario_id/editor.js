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
// The button on its way, the form as it left with it, and the buttons pressed
// since.
let sending = null;
let sent = null;
let waiting = [];
// Where the cursor goes back to once a block's settings are open again.
let cursor = null;

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

// The control of `form` named `name`.
function fieldOf(form, name) {
  return form.querySelector(`input[name="${name}"], textarea[name="${name}"], select[name="${name}"]`);
}

// What was typed since the form left, by field.
function typedSince() {
  const typed = new Map();
  for (const [name, value] of new FormData(root)) {
    if (value !== sent.get(name)) typed.set(name, value);
  }
  return typed;
}

// The flow's shape. A button that names a place in the flow (`add:root.1:…`)
// means that place on this shape only.
function flowOf(form) {
  return [...form.querySelectorAll("[data-step], [data-drop-slot]")]
    .map((node) => node.dataset.step ?? node.dataset.dropSlot)
    .join();
}

// Shows the editor's alert that says `what`, or none.
function say(what) {
  for (const alert of root.querySelectorAll("[data-press-alert]")) {
    alert.hidden = alert.dataset.pressAlert !== what;
  }
}

// Whether the editor says something went wrong.
function refused() {
  return [...root.querySelectorAll("[role=alert]"), ...noticesBeside(root)].some(
    (notice) => notice.matches("[role=alert]") && notice.checkVisibility(),
  );
}

// A button pressed while an answer is on its way goes once that answer is in,
// on the editor it brings. Not the same button again with nothing typed in
// between: that is a double click.
function hold(press) {
  const again =
    waiting.some((other) => other.press.op === press.op) ||
    (press.op === sending && !typedSince().size);
  if (!again) waiting.push({ press, flow: flowOf(root) });
}

// What was pressed goes on, unless the page is being left or the editor says
// something went wrong (to be read first: the next answer would replace it).
// A button that names a place in a flow that has changed meanwhile is refused.
function goOn() {
  const next = leaving || refused() ? null : waiting.shift();
  if (!next) {
    waiting = [];
  } else if (/^(add|move|drop):/.test(next.press.op) && next.flow !== flowOf(root)) {
    waiting = [];
    say("place");
  } else {
    send(null, next.press);
  }
}

async function send(submitter, extra) {
  const press = { ...extra };
  if (submitter?.name) press[submitter.name] = submitter.value;
  if (busy) {
    hold(press);
    return;
  }
  busy = true;
  sending = press.op;
  sent = new FormData(root);
  root.setAttribute("aria-busy", "true");
  say(null);
  const form = new FormData(root);
  // A block's settings sent again are all of them: none of the open block's go.
  if (press.settings_for) {
    for (const name of [...form.keys()]) if (name.startsWith("settings.")) form.delete(name);
  }
  for (const [key, value] of Object.entries(press)) form.set(key, value);
  try {
    // URL-encoded, as a plain form posts it.
    const response = await fetch(root.action, { method: "POST", body: new URLSearchParams(form) });
    // The redirect reloads the page: not over what was typed meanwhile.
    if (response.redirected && !typedSince().size) {
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
    // Settings sent again have nothing to say of their own: what the page says
    // stays.
    if (!press.settings_for) for (const old of noticesBeside(root)) old.remove();
    swap(document.importNode(next, true));
    for (const notice of notices) root.before(notice);
  } catch {
    say("failed");
  } finally {
    busy = false;
    root?.removeAttribute("aria-busy");
    goOn();
  }
}

// The flash notices of a page sit next to the editor's form: those of the
// page the server sent replace the ones shown.
function noticesBeside(form) {
  return [...form.parentElement.children].filter(
    (element) => element !== form && element.matches("[role=alert], [role=status]"),
  );
}

// A block's settings as the form holds them, to send again and open it back.
function settingsOf(block) {
  const press = { op: "apply", selected: block, settings_for: block };
  for (const [name, value] of new FormData(root)) {
    if (name.startsWith("settings.")) press[name] = value;
  }
  return press;
}

function swap(next) {
  // The answer was made without what was typed meanwhile, which stays. Settings
  // of a block the answer closed (a save closes it) are sent again to open it
  // back; those of a block that is gone go with it.
  const typed = typedSince();
  const block = fieldOf(root, "settings_for")?.value;
  const same = block === fieldOf(next, "settings_for")?.value;
  const kept = (name) => (same || !name.startsWith("settings.") ? fieldOf(next, name) : null);
  for (const [name, value] of typed) {
    const field = kept(name);
    if (field) field.value = value;
  }
  if (typed.size) next.setAttribute("data-dirty", "");
  // The field with the focus keeps it, and its cursor. What the answer wants
  // focused goes first, unless the merchant is typing.
  const active = document.activeElement;
  const back = cursor;
  cursor = null;
  const at =
    back ??
    (root.contains(active) && active.name
      ? { name: active.name, start: active.selectionStart, end: active.selectionEnd, scroll: active.scrollTop }
      : null);
  const field = at && (back ? fieldOf(next, at.name) : kept(at.name));
  const closed = !same && [...typed.keys()].some((name) => name.startsWith("settings."));
  if (closed && next.querySelector(`[data-step="${block}"]`)) {
    waiting.unshift({ press: settingsOf(block), flow: flowOf(next) });
    cursor = at;
  }
  root.replaceWith(next);
  root = next;
  enhance();
  const focus = (at && typed.has(at.name) && field) || root.querySelector("[autofocus]") || field;
  if (focus) focus.focus();
  if (focus && focus === field && at.start != null) {
    field.setSelectionRange(at.start, at.end);
    field.scrollTop = at.scroll;
  }
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
