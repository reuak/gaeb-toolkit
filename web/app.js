const form = document.querySelector("#convert-form");
const fileInput = document.querySelector("#pdf");
const fileDrop = document.querySelector("#file-drop");
const fileTitle = document.querySelector("#file-title");
const fileMeta = document.querySelector("#file-meta");
const jobPanel = document.querySelector("#job-panel");
const spinner = document.querySelector("#spinner");
const jobKicker = document.querySelector("#job-kicker");
const jobTitle = document.querySelector("#job-title");
const jobMessage = document.querySelector("#job-message");
const progressBar = document.querySelector("#progress-bar");
const downloadButton = document.querySelector("#download-button");
const newJobButton = document.querySelector("#new-job-button");
let pdfUploadLimit = 2 * 1024 * 1024;

document.querySelector("#year").textContent = new Date().getFullYear();


function applyPaidAppearance(status) {
  const isPro = status.plan === "pro";
  const credits = status.single_credits;
  document.body.classList.add("billing-active", isPro ? "billing-pro" : "billing-single");
  document.querySelector("#billing-banner").hidden = false;
  document.querySelector("#converter-kicker").textContent = isPro
    ? "GAEB Pro · Zugang aktiv"
    : "Bezahlte Konvertierung · Zugang aktiv";
  document.querySelector("#converter-title").textContent = isPro
    ? "Pro-Konvertierung starten"
    : "Gekauftes LV umwandeln";
  document.querySelector("#billing-banner-title").textContent = isPro
    ? "Ihr Pro-Zugang ist aktiv"
    : "Ihr Kauf ist erfolgreich aktiviert";
  document.querySelector("#billing-banner-copy").textContent = isPro
    ? "Erweiterte Uploads und Ihr Monatskontingent sind freigeschaltet."
    : "Ein Credit wird erst nach erfolgreicher Konvertierung verbraucht.";
  document.querySelector("#billing-credit").textContent = isPro
    ? "PRO"
    : `${credits} CREDIT${credits === 1 ? "" : "S"}`;
  document.querySelector("#trust-size").textContent = "Bis 25 MB freigeschaltet";
  document.querySelector("#trust-volume").textContent = isPro
    ? "100 Dokumente pro Monat"
    : `${credits} bezahlte Konvertierung${credits === 1 ? "" : "en"}`;
  document.querySelector("#trust-positions").textContent = "Erweiterter Positionsumfang";
  document.querySelector("#trust-retention").textContent = isPro
    ? "2 GB Dokumentenspeicher"
    : "Download für 7 Tage";
  document.querySelector("#convert-form .primary-button span").textContent = isPro
    ? "Pro-Konvertierung starten"
    : "Credit einsetzen und konvertieren";
  const currentCard = document.querySelector(isPro ? ".price-card.featured" : ".price-card:not(.featured)");
  if (currentCard) currentCard.classList.add("is-current");
}

async function initializeBilling() {
  const forms = document.querySelectorAll(".checkout-form");
  if (!forms.length) return;
  try {
    const statusResponse = await fetch("/api/billing/status");
    const status = await statusResponse.json();
    if (status.signed_in) {
      pdfUploadLimit = status.max_upload_bytes;
      fileMeta.textContent = `Maximal ${(pdfUploadLimit / 1024 / 1024).toFixed(0)} MB`;
      const label = status.plan === "pro"
        ? "Pro ist aktiv"
        : `${status.single_credits} Einzelkonvertierung${status.single_credits === 1 ? "" : "en"} verfügbar`;
      document.querySelector("#preise .eyebrow").textContent = label;
      applyPaidAppearance(status);
    }
    const response = await fetch("/api/billing/config");
    const config = await response.json();
    const money = (cents) => new Intl.NumberFormat("de-DE", {style:"currency",currency:"EUR"}).format(cents/100);
    document.querySelector("#single-price").textContent = money(config.single_net_cents);
    document.querySelector("#pro-price").textContent = money(config.pro_net_cents);
    if (config.offer_banner) { const banner=document.querySelector("#offer-banner"); banner.textContent=config.offer_banner; banner.hidden=false; }
    for (const form of forms) {
      const button = form.querySelector("button");
      if (!config.enabled) {
        button.disabled = true;
        form.querySelector(".checkout-note").textContent = "Bezahlbereich wird in Kürze freigeschaltet.";
      }
    }
  } catch (_) {
    for (const form of forms) form.querySelector("button").disabled = true;
  }

  const checkout = new URLSearchParams(window.location.search).get("checkout");
  if (checkout === "success") {
    document.querySelector("#preise").scrollIntoView({ behavior: "smooth" });
    document.querySelector("#preise .eyebrow").textContent = "Zahlung erfolgreich – Freischaltung wird geprüft";
  }

  for (const form of forms) {
    form.addEventListener("submit", async (event) => {
      event.preventDefault();
      const button = form.querySelector("button");
      const note = form.querySelector(".checkout-note");
      button.disabled = true;
      note.textContent = "Sicherer Stripe-Checkout wird geöffnet …";
      try {
        const response = await fetch("/api/billing/checkout", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ offer: form.dataset.offer, email: new FormData(form).get("email") }),
        });
        const data = await response.json();
        if (!response.ok) throw new Error(data.error || "Checkout konnte nicht geöffnet werden.");
        window.location.assign(data.url);
      } catch (error) {
        note.textContent = error.message;
        button.disabled = false;
      }
    });
  }
}

initializeBilling();

function showFile(file) {
  if (!file) return;
  fileTitle.textContent = file.name;
  fileMeta.textContent = `${(file.size / 1024 / 1024).toFixed(2)} MB`;
}

fileInput.addEventListener("change", () => showFile(fileInput.files[0]));
fileInput.addEventListener("change", () => {
  document.querySelector("#confirm-structure").value = "false";
  const label = document.querySelector("#convert-form .primary-button span");
  if (label.textContent.startsWith("Trotzdem")) {
    label.textContent = document.body.classList.contains("billing-pro")
      ? "Pro-Konvertierung starten"
      : document.body.classList.contains("billing-single")
        ? "Credit einsetzen und konvertieren"
        : "PDF in X83 konvertieren";
  }
});
for (const eventName of ["dragenter", "dragover"]) {
  fileDrop.addEventListener(eventName, (event) => {
    event.preventDefault();
    fileDrop.classList.add("is-dragging");
  });
}
for (const eventName of ["dragleave", "drop"]) {
  fileDrop.addEventListener(eventName, (event) => {
    event.preventDefault();
    fileDrop.classList.remove("is-dragging");
  });
}
fileDrop.addEventListener("drop", (event) => {
  const file = event.dataTransfer.files[0];
  if (!file) return;
  const transfer = new DataTransfer();
  transfer.items.add(file);
  fileInput.files = transfer.files;
  showFile(file);
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const file = fileInput.files[0];
  if (!file) return;
  if (file.size > pdfUploadLimit) {
    showError(`Die Datei ist größer als ${(pdfUploadLimit / 1024 / 1024).toFixed(0)} MB.`);
    return;
  }

  const submit = form.querySelector("button[type=submit]");
  submit.disabled = true;
  submit.setAttribute("aria-busy", "true");
  try {
    const response = await fetch("/api/convert", {
      method: "POST",
      body: new FormData(form),
    });
    const data = await response.json();
    if (response.status === 409) {
      document.querySelector("#confirm-structure").value = "true";
      showError(data.error);
      submit.querySelector("span").textContent = "Trotzdem umwandeln und Credit verwenden";
      submit.disabled = false;
      submit.removeAttribute("aria-busy");
      return;
    }
    if (!response.ok) throw new Error(data.error || "Upload fehlgeschlagen.");
    form.hidden = true;
    jobPanel.hidden = false;
    await pollJob(data.id, data.token);
    submit.removeAttribute("aria-busy");
  } catch (error) {
    showError(error.message);
    submit.disabled = false;
    submit.removeAttribute("aria-busy");
  }
});

async function pollJob(id, token) {
  let ticks = 0;
  while (ticks < 240) {
    await new Promise((resolve) => setTimeout(resolve, ticks === 0 ? 500 : 1500));
    const response = await fetch(`/api/jobs/${id}?token=${encodeURIComponent(token)}`);
    const data = await response.json();
    if (!response.ok) {
      showJobError(data.error || "Der Status konnte nicht geladen werden.");
      return;
    }
    if (data.status === "ready") {
      spinner.className = "spinner is-done";
      jobKicker.textContent = "Konvertierung abgeschlossen";
      jobTitle.textContent = "Ihre X83 ist bereit.";
      jobMessage.textContent = "Der Download ist für 24 Stunden verfügbar.";
      progressBar.style.width = "100%";
      downloadButton.href = data.download_url;
      downloadButton.hidden = false;
      newJobButton.hidden = false;
      return;
    }
    if (data.status === "emailed_with_warnings") {
      spinner.className = "spinner is-done";
      jobKicker.textContent = "Prüfentwurf versendet";
      jobTitle.textContent = "Bitte prüfen Sie Ihr E-Mail-Postfach.";
      jobMessage.textContent =
        data.error ||
        "Original-PDF, X83-Entwurf und Fehlerprotokoll wurden per E-Mail versendet.";
      progressBar.style.width = "100%";
      downloadButton.hidden = true;
      downloadButton.removeAttribute("href");
      newJobButton.hidden = false;
      return;
    }
    if (data.status === "failed") {
      showJobError(data.error || "Das LV konnte nicht konvertiert werden.");
      return;
    }
    ticks += 1;
    progressBar.style.width = `${Math.min(88, 30 + ticks * 2)}%`;
    if (data.status === "processing") {
      jobKicker.textContent = "PDF wird analysiert";
      jobTitle.textContent = "OZ und Positionen werden geprüft.";
    }
  }
  showJobError("Die Verarbeitung dauert ungewöhnlich lange. Bitte später erneut versuchen.");
}

function showError(message) {
  fileMeta.textContent = message;
  fileMeta.style.color = "var(--error)";
}

function showJobError(message) {
  form.hidden = true;
  jobPanel.hidden = false;
  downloadButton.hidden = true;
  downloadButton.removeAttribute("href");
  spinner.className = "spinner is-error";
  jobKicker.textContent = "Konvertierung nicht möglich";
  jobTitle.textContent = "Das PDF muss geprüft werden.";
  jobMessage.textContent = message;
  progressBar.style.width = "100%";
  progressBar.style.background = "#a53e35";
  newJobButton.hidden = false;
}

newJobButton.addEventListener("click", () => {
  window.location.reload();
});
