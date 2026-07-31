(async () => {
  const response = await fetch("/api/public-config");
  if (!response.ok) return;
  const config = await response.json();
  const services = {
    analytics: Boolean(config.google_tag_manager_id || config.google_analytics_id),
    marketing: Boolean(config.meta_pixel_id || config.klicktipp_pixel_url),
  };
  if (!services.analytics && !services.marketing) {
    document.querySelectorAll("[data-open-consent]").forEach((button) => {
      button.hidden = true;
    });
    return;
  }

  const storageKey = `gaeb-consent:${config.consent_version || "1"}`;
  let loaded = { analytics: false, marketing: false };

  function readChoice() {
    try {
      return JSON.parse(localStorage.getItem(storageKey) || "null");
    } catch (_) {
      return null;
    }
  }

  function injectScript(src) {
    const script = document.createElement("script");
    script.async = true;
    script.src = src;
    document.head.appendChild(script);
  }

  function updateGoogleConsent(choice) {
    if (!window.gtag) return;
    window.gtag("consent", "update", {
      analytics_storage: choice.analytics ? "granted" : "denied",
      ad_storage: choice.marketing ? "granted" : "denied",
      ad_user_data: choice.marketing ? "granted" : "denied",
      ad_personalization: choice.marketing ? "granted" : "denied",
    });
  }

  function loadGoogle(choice) {
    if (!services.analytics || loaded.analytics) return;
    loaded.analytics = true;
    window.dataLayer = window.dataLayer || [];
    window.gtag = window.gtag || function gtag() { window.dataLayer.push(arguments); };
    window.gtag("consent", "default", {
      analytics_storage: "denied",
      ad_storage: "denied",
      ad_user_data: "denied",
      ad_personalization: "denied",
    });
    updateGoogleConsent(choice);
    if (config.google_tag_manager_id) {
      window.dataLayer.push({ "gtm.start": Date.now(), event: "gtm.js" });
      injectScript(`https://www.googletagmanager.com/gtm.js?id=${encodeURIComponent(config.google_tag_manager_id)}`);
    }
    if (config.google_analytics_id) {
      injectScript(`https://www.googletagmanager.com/gtag/js?id=${encodeURIComponent(config.google_analytics_id)}`);
      window.gtag("js", new Date());
      window.gtag("config", config.google_analytics_id, { anonymize_ip: true });
    }
  }

  function loadMeta() {
    if (!config.meta_pixel_id) return;
    ((f, b, e, v, n, t, s) => {
      if (f.fbq) return;
      n = f.fbq = function fbq() { n.callMethod ? n.callMethod.apply(n, arguments) : n.queue.push(arguments); };
      if (!f._fbq) f._fbq = n;
      n.push = n;
      n.loaded = true;
      n.version = "2.0";
      n.queue = [];
      t = b.createElement(e);
      t.async = true;
      t.src = v;
      s = b.getElementsByTagName(e)[0];
      s.parentNode.insertBefore(t, s);
    })(window, document, "script", "https://connect.facebook.net/en_US/fbevents.js");
    window.fbq("consent", "grant");
    window.fbq("init", config.meta_pixel_id);
    window.fbq("track", "PageView");
  }

  function loadKlickTipp() {
    if (!config.klicktipp_pixel_url) return;
    const pixel = new Image(1, 1);
    pixel.alt = "";
    pixel.referrerPolicy = "strict-origin-when-cross-origin";
    pixel.src = config.klicktipp_pixel_url;
  }

  function apply(choice) {
    if (choice.analytics) loadGoogle(choice);
    updateGoogleConsent(choice);
    if (choice.marketing && !loaded.marketing) {
      loaded.marketing = true;
      loadMeta();
      loadKlickTipp();
    }
  }

  function save(choice) {
    try {
      localStorage.setItem(storageKey, JSON.stringify(choice));
    } catch (_) {
      // Die Auswahl gilt dann nur für den aktuellen Seitenaufruf.
    }
    const revokesLoadedService =
      (loaded.analytics && !choice.analytics) || (loaded.marketing && !choice.marketing);
    if (window.gtag && !choice.analytics) {
      window.gtag("consent", "update", { analytics_storage: "denied" });
    }
    if (window.fbq && !choice.marketing) window.fbq("consent", "revoke");
    if (revokesLoadedService) window.location.reload();
    else apply(choice);
  }

  function openDialog(force = false) {
    document.querySelector(".consent-backdrop")?.remove();
    const stored = readChoice() || {};
    const backdrop = document.createElement("div");
    backdrop.className = "consent-backdrop";
    backdrop.innerHTML = `
      <section class="consent-dialog" role="dialog" aria-modal="true" aria-labelledby="consent-title" tabindex="-1">
        <h2 id="consent-title">Datenschutz-Einstellungen</h2>
        <p>Notwendige Funktionen sind immer aktiv. Optionale Dienste laden wir erst nach Ihrer Zustimmung. Details stehen in der <a href="/datenschutz.html">Datenschutzerklärung</a>.</p>
        <div class="consent-options">
          <label class="consent-option">
            <input type="checkbox" checked disabled />
            <span><strong>Notwendig</strong><small>Konvertierung, Sicherheit und Speicherung Ihrer Auswahl.</small></span>
          </label>
          ${services.analytics ? `<label class="consent-option"><input id="consent-analytics" type="checkbox" ${stored.analytics ? "checked" : ""} /><span><strong>Statistik</strong><small>Google Analytics zur anonymisierten Reichweitenmessung.</small></span></label>` : ""}
          ${services.marketing ? `<label class="consent-option"><input id="consent-marketing" type="checkbox" ${stored.marketing ? "checked" : ""} /><span><strong>Marketing</strong><small>Meta Pixel und/oder KlickTipp, soweit konfiguriert.</small></span></label>` : ""}
        </div>
        <div class="consent-actions">
          <button type="button" data-consent="reject">Nur notwendige</button>
          <button type="button" data-consent="save">Auswahl speichern</button>
          <button class="consent-accept" type="button" data-consent="all">Alle akzeptieren</button>
        </div>
      </section>`;
    document.body.appendChild(backdrop);
    backdrop.querySelector(".consent-dialog").focus?.();
    backdrop.addEventListener("click", (event) => {
      const action = event.target.closest("[data-consent]")?.dataset.consent;
      if (!action) return;
      const choice = action === "all"
        ? { analytics: services.analytics, marketing: services.marketing }
        : action === "reject"
          ? { analytics: false, marketing: false }
          : {
              analytics: Boolean(backdrop.querySelector("#consent-analytics")?.checked),
              marketing: Boolean(backdrop.querySelector("#consent-marketing")?.checked),
            };
      backdrop.remove();
      save(choice);
    });
    if (force) backdrop.querySelector("button")?.focus();
  }

  document.querySelectorAll("[data-open-consent]").forEach((button) => {
    button.addEventListener("click", () => openDialog(true));
  });

  const stored = readChoice();
  if (stored) apply(stored);
  else openDialog();
})();
