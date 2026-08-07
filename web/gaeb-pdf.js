const form = document.querySelector("#gaeb-reader-form");
const input = document.querySelector("#gaeb");
const meta = document.querySelector("#gaeb-file-meta");
const note = document.querySelector("#gaeb-note");
const outputSelect = document.querySelector("#gaeb-output-format");
const checkoutForm = document.querySelector("#gaeb-pro-checkout");
const allowedExtensions = new Set(["d81", "d83", "p81", "p83", "x80", "x81", "x82", "x83", "x84", "x85", "x86", "x89", "xml"]);
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
      meta.textContent = "GAEB 90: D81/D83 · DA 2000: P81/P83 · DA XML: X80–X86, X89 · maximal 25 MB";
      note.textContent = "Diese Konvertierung zählt zu Ihrem gemeinsamen Pro-Monatskontingent.";
      const upsell = document.querySelector("#gaeb-pro-upsell");
      if (upsell) upsell.hidden = true;
      document.querySelector("#gaeb-pro-card")?.classList.add("is-current");
      const checkoutButton = checkoutForm?.querySelector("button");
      if (checkoutButton) {
        checkoutButton.disabled = true;
        checkoutButton.textContent = "GAEB Pro ist aktiv";
      }
    } else {
      document.querySelector("#free-fields input[type=email]").required = true;
      document.querySelector("#free-fields input[type=checkbox]").required = true;
    }
  } catch (_) {
    note.textContent = "Kontostatus konnte nicht geladen werden.";
  }
}

async function initializeCheckout() {
  if (!checkoutForm) return;
  const button = checkoutForm.querySelector("button");
  const checkoutNote = checkoutForm.querySelector(".checkout-note");
  try {
    const response = await fetch("/api/billing/config");
    const config = await response.json();
    if (!config.enabled) {
      button.disabled = true;
      checkoutNote.textContent = "Der Pro-Checkout wird in Kürze freigeschaltet.";
    }
  } catch (_) {
    button.disabled = true;
    checkoutNote.textContent = "Der Bezahlbereich konnte nicht geladen werden.";
  }

  checkoutForm.addEventListener("submit", async (event) => {
    event.preventDefault();
    button.disabled = true;
    checkoutNote.textContent = "Sicherer Stripe-Checkout wird geöffnet …";
    try {
      const response = await fetch("/api/billing/checkout", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ offer: "pro", email: new FormData(checkoutForm).get("email") }),
      });
      const data = await response.json();
      if (!response.ok) throw new Error(data.error || "Checkout konnte nicht geöffnet werden.");
      window.location.assign(data.url);
    } catch (error) {
      checkoutNote.textContent = error.message;
      button.disabled = false;
    }
  });
}

input.addEventListener("change", () => {
  const file = input.files[0];
  if (!file) return;
  const extension = extensionOf(file);
  document.querySelector("#gaeb-file-title").textContent = file.name;
  meta.textContent = allowedExtensions.has(extension)
    ? `${(file.size / 1024 / 1024).toFixed(2)} MB · ${extension.toUpperCase()}`
    : "Nicht unterstütztes Format. Bitte D81, D83, P81, P83, X80–X86, X89 oder XML wählen.";
});

outputSelect.addEventListener("change", () => {
  const label = outputSelect.value === "x83" ? "Als modernes X83 herunterladen" : "GAEB als PDF herunterladen";
  form.querySelector("button[type=submit] span").textContent = label;
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const file = input.files[0];
  if (!file) return;
  if (!allowedExtensions.has(extensionOf(file))) {
    note.textContent = "Bitte eine GAEB-Datei als D81, D83, P81, P83, X80–X86, X89 oder XML auswählen.";
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
    const url = URL.createObjectURL(await response.blob());
    const link = document.createElement("a");
    link.href = url;
    const outputFormat = new FormData(form).get("output_format") === "x83" ? "x83" : "pdf";
    link.download = `${file.name.replace(/\.[^.]+$/, "") || "leistungsverzeichnis"}.${outputFormat}`;
    link.click();
    URL.revokeObjectURL(url);
    note.textContent = "PDF wurde erstellt und heruntergeladen.";
  } catch (error) {
    note.textContent = error.message;
  } finally {
    button.disabled = false;
  }
});

init();
initializeCheckout();
