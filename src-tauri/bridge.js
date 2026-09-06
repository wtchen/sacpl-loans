/*
 * Bridge script — injected into the hidden WKWebView that loads
 * https://catalog.saclibrary.org/MyAccount/Home (Aspen by LSC).
 *
 * Every method returns a Promise that resolves to a JSON-serializable object.
 * Results are also mirrored into `window.__store[name]` so the Rust side can
 * poll them even if a page reload kills the pending promise.
 *
 * All fetch() calls are same-origin, so the Aspen session cookie is sent and
 * stored automatically in the webview's persistent cookie jar — that is what
 * makes the login survive app restarts.
 */
(function () {
  "use strict";

  const store = (window.__store = window.__store || {});
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  const wrap = (name, fn) => (...args) =>
    Promise.resolve()
      .then(() => fn(...args))
      .then((data) => {
        store[name] = { ok: true, data };
        return data;
      })
      .catch((e) => {
        store[name] = { ok: false, error: String((e && e.message) || e) };
        throw e;
      });

  const api = (url, opts) =>
    fetch(url, Object.assign({ credentials: "same-origin", headers: { Accept: "application/json" } }, opts || {}));

  /** Wait until the real catalog page (not a challenge page) is loaded. */
  function waitForPage(maxMs) {
    return (async () => {
      const deadline = Date.now() + (maxMs || 30000);
      while (Date.now() < deadline) {
        if (window.Globals && typeof window.Globals.loggedIn !== "undefined") return window.Globals;
        await sleep(1000);
      }
      return null;
    })();
  }

  const j = (u) => encodeURIComponent(u);

  function parseCheckoutsHtml(html) {
    const tpl = document.createElement("template");
    tpl.innerHTML = html || "";

    const DATE_RE =
      /\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Sept|Oct|Nov|Dec)[a-z]*\.?\s+\d{1,2}(?:,\s*\d{2,4})?|\d{1,2}\/\d{1,2}\/\d{2,4}|\d{4}-\d{2}-\d{2}\b/gi;

    const clean = (s) => String(s || "").replace(/\s+/g, " ").trim();

    // Each row has a `checkedOut-covers-column` with an
    // `img.listResultImage` pointing at the catalog's bookcover.php (or an
    // OverDrive CDN URL). Absolutize relative URLs against the bridge page.
    const extractCover = (row) => {
      const img =
        row.querySelector("img.listResultImage") ||
        row.querySelector(".checkedOut-covers-column img") ||
        row.querySelector("img[src*='bookcover']") ||
        row.querySelector("img");
      if (!img) return "";
      const src = img.getAttribute("data-src") || img.getAttribute("src") || "";
      if (!src || /^data:/i.test(src)) return "";
      try {
        return new URL(src, location.href).href;
      } catch (_) {
        return src;
      }
    };

    const extractDue = (row) => {
      const els = row.querySelectorAll(".result-label, .result-sublabel, .label, .result-info, th, td, span, div");
      for (const el of els) {
        const t = clean(el.textContent);
        if (/^(due|return date|due date|expires?|due by)\b/i.test(t) && t.length < 30) {
          const sib = el.nextElementSibling;
          if (sib) {
            const v = clean(sib.textContent);
            if (v && v.length < 60) return v;
          }
          const pv = el.parentElement;
          if (pv) {
            const vv = pv.querySelector(".result-value, .value");
            if (vv) {
              const v = clean(vv.textContent);
              if (v && v.length < 60) return v;
            }
          }
        }
      }
      const dates = (row.textContent || "").match(DATE_RE);
      return dates ? dates[dates.length - 1] : "";
    };

    const renewParams = (row) => {
      for (const a of row.querySelectorAll("a[onclick]")) {
        const oc = a.getAttribute("onclick") || "";
        let m = oc.match(/Account\.renewTitle\(\s*['"]([^'"]*)['"]\s*,\s*['"]([^'"]*)['"]\s*,\s*([^)]+?)\s*\)/);
        if (m) {
          return {
            kind: "ils",
            patronId: m[1],
            recordId: m[2],
            renewIndicator: String(m[3]).replace(/^['"]|['"]$/g, ""),
            renewable: true,
          };
        }
        m = oc.match(/OverDrive\.renewCheckout\(\s*['"]([^'"]*)['"]\s*,\s*['"]([^'"]*)['"]\s*\)/);
        if (m) {
          return {
            kind: "overdrive",
            patronId: m[1],
            recordId: m[2],
            renewIndicator: "",
            renewable: true,
          };
        }
      }
      return { kind: "", patronId: "", recordId: "", renewIndicator: "", renewable: false };
    };

    // The title anchor links straight to the catalog record page
    // (/Record/.b… or /OverDrive/<id>/Home), used for "open in catalog".
    const extractUrl = (titleEl) => {
      const href = titleEl.getAttribute("href") || "";
      if (!href) return "";
      try {
        return new URL(href, location.href).href;
      } catch (_) {
        return href;
      }
    };

    // Libby (OverDrive) items renew through the Libby app, not the ILS renew
    // flow. Rows without a renew anchor still have kind === "", so detect
    // them from the row class, the record link, or the Format value instead.
    const isLibby = (row, titleHref) => {
      if (/overdrive/i.test(String(row.className || ""))) return true;
      if (/\/OverDrive\//i.test(String(titleHref || ""))) return true;
      for (const v of row.querySelectorAll(".result-value")) {
        if (/\blibby\b|overdrive/i.test(clean(v.textContent))) return true;
      }
      return false;
    };

    // Each checkout renders as a `div.result` row containing a `.result-title`.
    const items = [];
    for (const row of tpl.content.querySelectorAll("div.result")) {
      const titleEl = row.querySelector(".result-title");
      if (!titleEl) continue;
      items.push({
        title: clean(titleEl.textContent) || "Untitled item",
        due: extractDue(row),
        cover: extractCover(row),
        url: extractUrl(titleEl),
        libby: isLibby(row, titleEl.getAttribute("href")),
        overdue: /overdue|bg-overdue/i.test(String(row.className) + " " + String(titleEl.className || "")),
        ...renewParams(row),
      });
    }
    return items;
  }

  /**
   * Fetch the patron's checkouts and parse them into structured items.
   *
   * `refreshCheckouts=true` forces the server to re-sync from the ILS.
   * Without it the catalog can return a cached "Loading..." placeholder
   * (right after login, the ILS sync runs in the background), which would
   * look like "no items checked out" — so we retry while the catalog
   * reports the sync as still in flight.
   *
   * A dead session is reported as `loggedOut: true`: the AJAX handler
   * bounces signed-out requests to the sign-in page (a redirect, or the
   * login form in the response body), so the caller can re-authenticate.
   */
  async function fetchCheckouts() {
    // Debug simulations (Settings ▸ Developer Settings ▸ Debug Events): a
    // one-shot flag that makes this fetch return the simulated outcome
    // instead of hitting the network. Consumed on first use.
    const sim = window.__sim || null;
    if (sim) {
      window.__sim = null;
      if (sim === "http403") {
        return { success: false, message: "Catalog returned HTTP 403", items: [] };
      }
      if (sim === "http500") {
        return { success: false, message: "Catalog returned HTTP 500", items: [] };
      }
      if (sim === "unreachable") {
        throw new Error("Simulated network failure: could not reach the catalog");
      }
      if (sim === "syncing") {
        return {
          success: true, message: "", lastLoaded: "Loading...", count: 0,
          items: [], stillSyncing: true, simulated: true,
        };
      }
      if (sim === "loggedOut") {
        return { success: false, loggedOut: true, message: "Your library session has ended.", items: [] };
      }
      if (sim === "changedData") {
        const fake = (n) => ({
          title: `Simulated Book ${n} (debug)`, due: n % 3 ? "Oct 1, 2026" : "Yesterday",
          cover: "", url: "", libby: n % 4 === 0, overdue: n % 3 === 0, kind: "",
          patronId: "", recordId: "", renewIndicator: "", renewable: false,
        });
        return {
          success: true, message: "", lastLoaded: "simulated", count: 30, simulated: true,
          items: Array.from({ length: 30 }, (_, i) => fake(i + 1)),
        };
      }
    }

    let attempts = 0;
    // eslint-disable-next-line no-constant-condition
    while (true) {
      attempts++;
      const res = await api("/MyAccount/AJAX?method=getCheckouts&source=all&refreshCheckouts=true");
      if (!res.ok) {
        return { success: false, message: "Catalog returned HTTP " + res.status, items: [] };
      }
      const body = await res.text();
      // Session expiry: bounced back to the sign-in page.
      if (res.redirected || /id="loginForm"/.test(body)) {
        return { success: false, loggedOut: true, message: "Your library session has ended.", items: [] };
      }
      let data;
      try {
        data = JSON.parse(body);
      } catch (_) {
        return { success: false, message: "The catalog returned an unexpected response.", items: [] };
      }
      if (!data.success) {
        const m = String(data.message || "");
        if (/log\s?in|sign\s?in|session|expired/i.test(m)) {
          return { success: false, loggedOut: true, message: "Your library session has ended.", items: [] };
        }
        return { success: false, message: m || "Failed to load checkouts", items: [] };
      }
      const html = data.checkouts || "";
      const items = parseCheckoutsHtml(html);
      const lastLoaded = data.checkoutInfoLastLoaded || "";
      const stillLoading = /loading/i.test(lastLoaded);
      if (attempts >= 10 || !stillLoading) {
        return {
          success: true,
          message: "",
          lastLoaded,
          count: items.length,
          items,
          attempts,
          stillSyncing: stillLoading && items.length === 0,
          rawHtmlLength: html.length,
          rawSnippet: html.slice(0, 3000),
        };
      }
      await sleep(3000);
    }
  }

  window.__bridge = {
    /** Login state + whether the catalog page is loaded. */
    status: wrap("status", async () => {
      const g = await waitForPage(2500);
      if (!g) return { ready: false, loggedIn: false, loginError: "" };
      let loginError = "";
      if (!g.loggedIn) {
        const el = document.getElementById("loginError");
        if (el) loginError = (el.textContent || "").trim();
      }
      return { ready: true, loggedIn: !!g.loggedIn, loginError };
    }),

    /**
     * Login by faithfully reproducing the catalog's own login form
     * submission: POST /MyAccount/Home with every field the live form
     * contains (hidden ones included) plus the clicked submit button.
     *
     * (Aspen's AJAX loginUser endpoint is broken on this installation — it
     * crashes with an LDAP error — so the form POST is the reliable path.)
     */
    doLogin: wrap("doLogin", async (username, password) => {
      const body = new URLSearchParams();
      let fields = "";
      const form = document.getElementById("loginForm");
      if (form) {
        for (const [k, v] of new FormData(form)) {
          if (typeof v === "string") body.set(k, v);
        }
        fields = [...form.querySelectorAll("input,button,select")]
          .map((el) => (el.type || el.tagName.toLowerCase()) + ":" + (el.name || el.id || "?"))
          .join(",");
      }
      body.set("username", username);
      body.set("password", password);
      // Ask the catalog to keep the session alive as long as it can.
      body.set("rememberMe", "on");
      // A native submission includes the clicked button; FormData(form) does
      // not, so set it explicitly.
      body.set("submit", "Login");

      const bodySent = body.toString();

      let r;
      try {
        const res = await fetch("/MyAccount/Home", {
          method: "POST",
          credentials: "same-origin",
          headers: { "Content-Type": "application/x-www-form-urlencoded; charset=UTF-8" },
          body: body.toString(),
        });
        if (!res.ok) {
          r = { ok: false, error: "Catalog returned HTTP " + res.status, fields, bodySent };
        } else {
          const html = await res.text();
          let err = "";
          let stillHasForm = false;
          try {
            const doc = new DOMParser().parseFromString(html, "text/html");
            const errEl = doc.getElementById("loginError");
            if (errEl) err = (errEl.textContent || "").trim();
            stillHasForm = !!doc.getElementById("loginForm");
          } catch (_) {}
          if (!err && stillHasForm) err = "Sign in failed. Check your library ID and PIN.";
          r = { ok: !stillHasForm, error: err, fields, htmlLen: html.length, bodySent };
        }
      } catch (e) {
        r = { ok: false, error: String((e && e.message) || e), fields };
      }
      try { sessionStorage.setItem("__lib_login_result", JSON.stringify(r)); } catch (_) {}
      if (r.ok) {
        // Reload so Globals.loggedIn (and the whole account UI) refreshes.
        setTimeout(() => {
          try { location.reload(); } catch (_) {}
        }, 150);
      }
      return r;
    }),

    /** Sign out and reload. */
    logout: wrap("logout", async () => {
      let ok = false;
      try {
        await api("/MyAccount/Logout");
        ok = true;
      } catch (_) {}
      try { sessionStorage.setItem("__lib_logout_result", JSON.stringify({ ok })); } catch (_) {}
      setTimeout(() => {
        try { location.reload(); } catch (_) {}
      }, 150);
      return { ok };
    }),

    checkouts: wrap("checkouts", fetchCheckouts),

    // Same fetch, stored under its own key so an hourly background refresh
    // from Rust can't interleave with a manual Refresh from the panel (both
    // sides poll window.__store by method name).
    checkoutsBg: wrap("checkoutsBg", fetchCheckouts),

    /**
     * Renew one title. kind routes to the right catalog flow: physical items
     * use the ILS renewCheckout, OverDrive (Libby) ebooks use the OverDrive
     * AJAX endpoint. Handles the confirm-renewal-fee step automatically
     * (but never accepts a paid renewal).
     */
    renewOne: wrap("renewOne", async (kind, patronId, recordId, renewIndicator) => {
      if (kind === "overdrive") {
        const url = "/OverDrive/AJAX?method=renewCheckout&patronId=" + j(patronId) +
          "&overDriveId=" + j(recordId);
        const res = await api(url);
        if (!res.ok) return { success: false, title: "Renewal failed", message: "Catalog returned HTTP " + res.status, renewed: 0 };
        const data = await res.json();
        return { success: !!data.success, title: data.title || (data.success ? "Renewed" : "Could not renew"), message: data.message || "", renewed: data.success ? 1 : 0 };
      }
      const base =
        "/MyAccount/AJAX?method=renewCheckout&patronId=" + j(patronId) +
        "&recordId=" + j(recordId) + "&renewIndicator=" + j(renewIndicator);
      let res = await api(base);
      if (!res.ok) return { success: false, title: "Renewal failed", message: "Catalog returned HTTP " + res.status, renewed: 0 };
      let data = await res.json();
      if (data.modalButtons) {
        const modalBody = String(data.modalBody || "");
        if (/fee|cost|\$|charge/i.test(modalBody)) {
          // Never auto-accept a paid renewal on the patron's behalf.
          return {
            success: false,
            title: data.title || "Confirmation required",
            message: "This renewal may involve a fee. Please confirm on the library site: " + modalBody,
            renewed: 0,
          };
        }
        res = await api(base + "&confirmedRenewal=true");
        data = await res.json();
      }
      const strip = (s) => String(s || "").replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
      return {
        success: !!data.success,
        title: strip(data.title) || (data.success ? "Renewed" : "Could not renew"),
        message: strip(data.message || data.modalBody),
        renewed: data.renewed || 0,
      };
    }),

    /** Renew every renewable checkout. */
    renewAll: wrap("renewAll", async () => {
      let res = await api("/MyAccount/AJAX?method=renewAll");
      if (!res.ok) return { success: false, title: "Renewal failed", message: "Catalog returned HTTP " + res.status, renewed: 0 };
      let data = await res.json();
      if (data.modalButtons) {
        res = await api("/MyAccount/AJAX?method=renewAll&confirmedRenewal=true");
        data = await res.json();
      }
      const strip = (s) => String(s || "").replace(/<[^>]*>/g, " ").replace(/\s+/g, " ").trim();
      return {
        success: !!data.success,
        title: strip(data.title) || (data.success ? "Renewals complete" : "Could not renew"),
        message: strip(data.message || data.modalBody),
        renewed: data.renewed || 0,
      };
    }),
  };
})();
