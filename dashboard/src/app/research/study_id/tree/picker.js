// The category tree's picker without a page load. A row's buttons are a
// plain form and the tabs and pages of the list plain links: here each is
// sent in the background, and what the server renders of the page takes
// the place of what is shown. Without this script the page loads again.

const said = document.querySelector("[data-picker-said]");
// The keywords sent from « To choose » whose answer has not come: their
// rows stay out of sight, whatever an earlier answer still lists.
const leaving = new Set();
let queue = Promise.resolve();
let waiting = 0;
let hiding;

const picker = () => document.querySelector("[data-picker]");
const rowsOf = (section) => [...section.querySelectorAll("tbody tr")].filter((row) => !row.hidden);

function say(text, error) {
  clearTimeout(hiding);
  said.textContent = text;
  said.toggleAttribute("data-error", Boolean(error));
  said.hidden = !text;
  if (text) {
    hiding = setTimeout(() => (said.hidden = true), 6000);
  }
}

function busy(more) {
  waiting += more;
  if (waiting > 0) {
    picker().setAttribute("aria-busy", "true");
  } else {
    picker().removeAttribute("aria-busy");
  }
}

// What the server said of the move, taken out of the page it rendered:
// it is said here, not at the top of a page nobody scrolled back to.
function noticeOf(next) {
  const notice = next.querySelector("[data-picker-notice]");
  const error = notice?.querySelector('[role="alert"]:not([data-auth-error])');
  const message = notice?.querySelector('[role="status"]');
  const text = (error ?? message)?.textContent.trim() ?? "";
  error?.remove();
  message?.remove();
  return { text, error: Boolean(error) };
}

// Puts the page as rendered in the place of the one shown. A part that
// reads the same stays, with whatever was being typed in it.
function swap(next) {
  const now = picker();
  const before = [...now.children];
  const after = [...next.children];
  const alike =
    before.length === after.length &&
    before.every((part, at) => part.tagName === after[at].tagName && part.id === after[at].id);
  if (!alike) {
    now.replaceChildren(...after);
  } else {
    before.forEach((part, at) => {
      if (part.outerHTML !== after[at].outerHTML) {
        part.replaceWith(after[at]);
      }
    });
  }
  for (const row of now.querySelectorAll("tr[data-kept]")) {
    row.hidden = leaving.has(row.dataset.kept);
  }
}

// Where a move was made, to stand there again once the page is swapped.
function standing(form) {
  const section = form.closest("section");
  const row = form.closest("tr");
  return {
    section: section.id,
    keyword: form.elements.keyword.value,
    at: rowsOf(section).indexOf(row),
    top: row.getBoundingClientRect().top,
  };
}

// The row to stand on after a move: the keyword's own if it is still in
// that part of the page, else the one that took its place.
function rowAfter(stood) {
  const section = document.getElementById(stood.section);
  if (!section) {
    return null;
  }
  const rows = rowsOf(section);
  const own = rows.find((row) => row.querySelector("[data-place]")?.elements.keyword.value === stood.keyword);
  return own ?? rows[Math.min(stood.at, rows.length - 1)] ?? section;
}

// Keeps `target` where the eye was (`top`) and gives it the focus the
// swap took away.
function settle(target, top) {
  if (!target) {
    return;
  }
  const row = target.matches("tr");
  if (row) {
    scrollBy(0, target.getBoundingClientRect().top - top);
  }
  const control = row ? target.querySelector("button, a") : target;
  (control ?? target.closest("section"))?.focus({ preventScroll: true });
}

async function rendered(response) {
  if (!response.ok) {
    return null;
  }
  const page = new DOMParser().parseFromString(await response.text(), "text/html");
  return page.querySelector("[data-picker]");
}

// The address the page is at, without what a redirect said in it.
function settled(url) {
  const at = new URL(url);
  at.searchParams.delete("error");
  at.searchParams.delete("message");
  return at;
}

async function move(action, body, stood, kept) {
  try {
    const response = await fetch(action, { method: "POST", body, headers: { Accept: "text/html" } });
    const next = await rendered(response);
    if (!next) {
      // Not this page: signed out, or the study is gone.
      location.assign(response.url);
      return;
    }
    leaving.delete(kept);
    const notice = noticeOf(next);
    swap(next);
    history.replaceState(null, "", settled(response.url));
    say(notice.text, notice.error);
    settle(rowAfter(stood), stood.top);
  } catch {
    leaving.delete(kept);
    for (const row of picker().querySelectorAll("tr[data-kept]")) {
      row.hidden = leaving.has(row.dataset.kept);
    }
    say("That could not be sent. Check your connection and try again.", true);
  }
}

async function open(url) {
  const top = document.getElementById("kept").getBoundingClientRect().top;
  try {
    const next = await rendered(await fetch(url, { headers: { Accept: "text/html" } }));
    if (!next) {
      location.assign(url);
      return;
    }
    noticeOf(next);
    swap(next);
    history.replaceState(null, "", settled(url));
    const list = document.getElementById("kept");
    scrollBy(0, list.getBoundingClientRect().top - top);
    (list.querySelector('[aria-current="page"]') ?? list).focus({ preventScroll: true });
  } catch {
    location.assign(url);
  }
}

function later(task) {
  busy(1);
  queue = queue.then(task).finally(() => busy(-1));
}

addEventListener("submit", (event) => {
  const form = event.target;
  if (!said || !form.matches?.("[data-picker] form[data-place]")) {
    return;
  }
  event.preventDefault();
  const body = new URLSearchParams(new FormData(form, event.submitter));
  const stood = standing(form);
  const row = form.closest("tr[data-kept]");
  const kept = row?.dataset.kept;
  say("");
  if (row) {
    // It leaves the list at once; the answer says where it went.
    leaving.add(kept);
    row.hidden = true;
  }
  later(() => move(form.action, body, stood, kept));
});

addEventListener("click", (event) => {
  const link = event.target.closest?.("[data-picker] #kept a[href]");
  if (!said || !link || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) {
    return;
  }
  event.preventDefault();
  say("");
  later(() => open(link.href));
});
