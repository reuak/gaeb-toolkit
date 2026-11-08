const form = document.querySelector("#gaeb-reader-form");
const input = document.querySelector("#gaeb");
const meta = document.querySelector("#gaeb-file-meta");
const note = document.querySelector("#gaeb-note");
const outputSelect = document.querySelector("#gaeb-output-format");
const checkoutForms = document.querySelectorAll(".checkout-form");
const allowedExtensions = new Set(["d81", "d83", "d84", "p81", "p83", "p84", "x80", "x81", "x82", "x83", "x84", "x85", "x86", "x89", "xml"]);
let limit = 2 * 1024 * 1024;

document.querySelector("#year").textContent = new Date().getFullYear();

function extensionOf(file) {
  return file.name.includes(".") ? file.name.split(".").pop().toLowerCase() : "";
}

async function init() {
  try {
    const response = await fetch("/api/billing/status");
    const status = await response.json();
    if (status.signed_in && status.plan === "pro") {
      limit = status.max_upload_bytes;
      document.body.classList.add("billing-active", "billing-pro");
      document.querySelector("#free-fields").hidden = true;
      document.querySelectorAll("#free-fields input").forEach((element) => {
        element.required = false;
      });
      document.querySelector("#gaeb-kicker").textContent = "GAEB Pro · Zugang aktiv";
      document.querySelector("#gaeb-limit").textContent = "Bis 25 MB pro Datei";
      document.querySelector("#gaeb-volume").textContent = "100 Konvertierungen pro Monat · beide Richtungen";
      meta.textContent = "GAEB 90: D81/D83/D84 · DA 2000: P81/P83/P84 · DA XML: X80–X86, X89 · maximal 25 MB";
      note.textContent = "Diese Konvertierung zählt zu Ihrem gemeinsamen Pro-Monatskontingent.";
      const upsell = document.querySelector("#gaeb-pro-upsell");
      if (upsell) upsell.hidden = true;
      document.querySelector("#gaeb-pro-card")?.classList.add("is-current");
      checkoutForms.forEach((checkoutForm) => {
        const checkoutButton = checkoutForm.querySelector("button");
        checkoutButton.disabled = true;
        checkoutButton.textContent = "GAEB Pro ist aktiv";
      });
    } else {
      document.querySelector("#free-fields input[type=email]").required = true;
      document.querySelector("#free-fields input[type=checkbox]").required = true;
    }
  } catch (_) {
    note.textContent = "Kontostatus konnte nicht geladen werden.";
  }
}

async function initializeCheckout() {
  if (!checkoutForms.length) return;
  try {
    const response = await fetch("/api/billing/config");
    const config = await response.json();
    if (!config.enabled) {
      checkoutForms.forEach((checkoutForm) => {
        checkoutForm.querySelector("button").disabled = true;
        checkoutForm.querySelector(".checkout-note").textContent = "Der Pro-Checkout wird in Kürze freigeschaltet.";
      });
    }
  } catch (_) {
    checkoutForms.forEach((checkoutForm) => {
      checkoutForm.querySelector("button").disabled = true;
      checkoutForm.querySelector(".checkout-note").textContent = "Der Bezahlbereich konnte nicht geladen werden.";
    });
  }

  checkoutForms.forEach((checkoutForm) => checkoutForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    const button = checkoutForm.querySelector("button");
    const checkoutNote = checkoutForm.querySelector(".checkout-note");
    const emailInput = checkoutForm.querySelector('input[type="email"]');
    if (!emailInput?.checkValidity()) {
      checkoutNote.textContent = "Bitte eine gültige E-Mail-Adresse eingeben.";
      emailInput?.focus();
      return;
    }
    button.disabled = true;
    checkoutNote.textContent = "Sicherer Stripe-Checkout wird geöffnet …";
    try {
      const response = await fetch("/api/billing/checkout", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ offer: checkoutForm.dataset.offer, email: new FormData(checkoutForm).get("email"), password: new FormData(checkoutForm).get("password") || null }),
      });
      const data = await response.json();
      if (!response.ok) throw new Error(data.error || "Checkout konnte nicht geöffnet werden.");
      window.gaebTrackEvent?.("checkout_started", { offer: checkoutForm.dataset.offer, source: "gaeb_to_pdf" });
      window.location.assign(data.url);
    } catch (error) {
      checkoutNote.textContent = error.message;
      button.disabled = false;
    }
  }));
}

input.addEventListener("change", () => {
  const file = input.files[0];
  if (!file) return;
  const extension = extensionOf(file);
  document.querySelector("#gaeb-file-title").textContent = file.name;
  meta.textContent = allowedExtensions.has(extension)
    ? `${(file.size / 1024 / 1024).toFixed(2)} MB · ${extension.toUpperCase()}`
    : "Nicht unterstütztes Format. Bitte D81, D83, D84, P81, P83, P84, X80–X86, X89 oder XML wählen.";
});

outputSelect.addEventListener("change", () => {
  const labels = {
    x83: "Als modernes X83 herunterladen",
    p84: "Als GAEB-2000-P84 herunterladen",
    d84: "Als GAEB-90-D84 herunterladen",
    pdf: "GAEB als PDF herunterladen",
  };
  const label = labels[outputSelect.value] || labels.pdf;
  form.querySelector("button[type=submit] span").textContent = label;
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const file = input.files[0];
  if (!file) return;
  if (!allowedExtensions.has(extensionOf(file))) {
    note.textContent = "Bitte eine GAEB-Datei als D81, D83, D84, P81, P83, P84, X80–X86, X89 oder XML auswählen.";
    return;
  }
  if (file.size > limit) {
    meta.textContent = `Die Datei ist größer als ${limit / 1024 / 1024} MB.`;
    return;
  }

  const button = form.querySelector("button[type=submit]");
  button.disabled = true;
  note.textContent = "PDF wird erstellt …";
  try {
    const response = await fetch("/api/gaeb-to-pdf", { method: "POST", body: new FormData(form) });
    if (!response.ok) {
      const data = await response.json();
      throw new Error(data.error || "Konvertierung fehlgeschlagen.");
    }
    if (!document.body.classList.contains("billing-active")) {
      window.gaebTrackEvent?.("free_conversion_success", { direction: "gaeb_to_pdf" });
    }
    const url = URL.createObjectURL(await response.blob());
    const link = document.createElement("a");
    link.href = url;
    const outputFormat = new FormData(form).get("output_format") === "x83" ? "x83" : "pdf";
    link.download = `${file.name.replace(/\.[^.]+$/, "") || "leistungsverzeichnis"}.${outputFormat}`;
    link.click();
    URL.revokeObjectURL(url);
    note.textContent = "PDF wurde erstellt und heruntergeladen.";
  } catch (error) {
    const freeLimitReached = error.message.includes("einmalige kostenlose Konvertierung");
    note.classList.toggle("is-error", freeLimitReached);
    if (freeLimitReached) {
      note.innerHTML = '<strong>Die kostenlose Konvertierung wurde bereits genutzt.</strong><span>Mit GAEB Pro können Sie sofort weiterarbeiten.</span><a href="#preise">Pro-Angebote ansehen →</a>';
      note.scrollIntoView({ behavior: "smooth", block: "center" });
    } else {
      note.textContent = error.message;
    }
  } finally {
    button.disabled = false;
  }
});

init();
initializeCheckout();
