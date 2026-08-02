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
const downloadX84Button = document.querySelector("#download-x84-button");
const reviewButton = document.querySelector("#review-button");
const newJobButton = document.querySelector("#new-job-button");
const preflightPanel = document.querySelector("#preflight-progress");
const preflightStatus = document.querySelector("#preflight-status");
const preflightDetail = document.querySelector("#preflight-detail");
const preflightPercent = document.querySelector("#preflight-percent");
const preflightBar = document.querySelector("#preflight-bar");
const resetUploadButton = document.querySelector("#reset-upload-button");
let activeUploadController = null;
let preflightTimer = null;
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
      if (status.plan === "pro" || status.single_credits > 0) {
        const label = status.plan === "pro"
          ? "Pro ist aktiv"
          : `${status.single_credits} Einzelkonvertierung${status.single_credits === 1 ? "" : "en"} verfügbar`;
        document.querySelector("#preise .eyebrow").textContent = label;
        applyPaidAppearance(status);
      }
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

async function loadReviews(){try{const r=await fetch("/api/reviews/public"),reviews=await r.json();if(!r.ok||!reviews.length)return;const section=document.querySelector("#bewertungen"),grid=document.querySelector("#public-reviews");grid.replaceChildren(...reviews.map(review=>{const article=document.createElement("article");article.className="review-card";const stars=document.createElement("strong");stars.textContent=`${"★".repeat(review.rating)}${"☆".repeat(5-review.rating)}`;const text=document.createElement("p");text.textContent=review.text||"Bewertung ohne Kommentar";const date=document.createElement("small");date.textContent=`Verifizierte Konvertierung · ${new Date(review.created_at).toLocaleDateString("de-DE")}`;article.append(stars,text,date);return article;}));section.hidden=false;}catch(_){}}loadReviews();

function showFile(file) {
  if (!file) return;
  fileTitle.textContent = file.name;
  fileMeta.textContent = `${(file.size / 1024 / 1024).toFixed(2)} MB`;
}

fileInput.addEventListener("change", () => showFile(fileInput.files[0]));
fileInput.addEventListener("change", () => {
  document.querySelector("#confirm-structure").value = "false";
  resetUploadButton.hidden = true;
  preflightPanel.hidden = true;
  fileMeta.style.color = "";
  const label = document.querySelector("#convert-form .primary-button span");
  if (label.textContent.startsWith("Trotzdem")) {
    label.textContent = document.body.classList.contains("billing-pro")
      ? "Pro-Konvertierung starten"
      : document.body.classList.contains("billing-single")
        ? "Credit einsetzen und konvertieren"
        : "PDF in X83 konvertieren";
  }
});

function normalSubmitLabel() {
  return document.body.classList.contains("billing-pro")
    ? "Pro-Konvertierung starten"
    : document.body.classList.contains("billing-single")
      ? "Credit einsetzen und konvertieren"
      : "PDF in X83 konvertieren";
}

function updatePreflight(percent, status, detail) {
  preflightBar.style.width = `${percent}%`;
  preflightPercent.textContent = `${percent} %`;
  preflightStatus.textContent = status;
  preflightDetail.textContent = detail;
}

function startPreflightProgress(canCancel) {
  clearInterval(preflightTimer);
  preflightPanel.hidden = false;
  resetUploadButton.hidden = !canCancel;
  updatePreflight(10, "Datei wird hochgeladen …", "Das PDF wird verschlüsselt übertragen.");
  const startedAt = Date.now();
  preflightTimer = setInterval(() => {
    const seconds = (Date.now() - startedAt) / 1000;
    if (seconds > 18) updatePreflight(88, "GAEB-Entwurf wird vorbereitet …", "Die erkannten Positionen werden für den Export validiert.");
    else if (seconds > 9) updatePreflight(70, "LV-Struktur wird geprüft …", "Ordnungszahlen, Bereiche, Mengen und Positionen werden ausgewertet.");
    else if (seconds > 3) updatePreflight(42, "Textebene und deutsche OCR werden geprüft …", "Bei Scan-PDFs kann dieser Schritt etwas länger dauern.");
    else updatePreflight(22, "PDF wird geöffnet …", "Seiten und Dokumentstruktur werden gelesen.");
  }, 700);
}

function stopPreflightProgress() {
  clearInterval(preflightTimer);
  preflightTimer = null;
  preflightPanel.hidden = true;
}

function resetUpload() {
  activeUploadController?.abort();
  activeUploadController = null;
  stopPreflightProgress();
  document.querySelector("#confirm-structure").value = "false";
  fileInput.value = "";
  fileTitle.textContent = "PDF auswählen oder hier ablegen";
  fileMeta.textContent = `Maximal ${(pdfUploadLimit / 1024 / 1024).toFixed(0)} MB`;
  fileMeta.style.color = "";
  form.querySelector("button[type=submit] span").textContent = normalSubmitLabel();
  const submit = form.querySelector("button[type=submit]");
  submit.disabled = false;
  submit.removeAttribute("aria-busy");
  resetUploadButton.hidden = true;
  fileInput.click();
}

resetUploadButton.addEventListener("click", resetUpload);
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
  document.querySelector("#confirm-structure").value = "false";
  form.querySelector("button[type=submit] span").textContent = normalSubmitLabel();
  resetUploadButton.hidden = true;
  fileMeta.style.color = "";
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
  const confirmed = document.querySelector("#confirm-structure").value === "true";
  submit.disabled = true;
  submit.setAttribute("aria-busy", "true");
  activeUploadController = new AbortController();
  startPreflightProgress(!confirmed);
  try {
    const response = await fetch("/api/convert", {
      method: "POST",
      body: new FormData(form),
      signal: activeUploadController.signal,
    });
    const data = await response.json();
    if (response.status === 409) {
      stopPreflightProgress();
      document.querySelector("#confirm-structure").value = "true";
      showError(data.error);
      submit.querySelector("span").textContent = "Trotzdem umwandeln und Credit verwenden";
      submit.disabled = false;
      submit.removeAttribute("aria-busy");
      resetUploadButton.hidden = false;
      return;
    }
    if (!response.ok) throw new Error(data.error || "Upload fehlgeschlagen.");
    updatePreflight(100, "Vorprüfung abgeschlossen", "Der Konvertierungsauftrag wurde gestartet.");
    stopPreflightProgress();
    form.hidden = true;
    jobPanel.hidden = false;
    await pollJob(data.id, data.token);
    submit.removeAttribute("aria-busy");
  } catch (error) {
    stopPreflightProgress();
    if (error.name === "AbortError") return;
    showError(error.message);
    submit.disabled = false;
    submit.removeAttribute("aria-busy");
  } finally {
    activeUploadController = null;
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
      const options = data.download_options || [];
      const x83 = options.find((option) => option.format === "x83");
      const x84 = options.find((option) => option.format === "x84");
      jobTitle.textContent = x84 ? "Ihre X83 und X84 sind bereit." : "Ihre X83 ist bereit.";
      jobMessage.textContent = x84
        ? "Die X83 enthält die Ausschreibung ohne Preise; die X84 übernimmt die erkannten Angebotspreise."
        : "Es wurden keine Angebotspreise erkannt. Der Download ist für 24 Stunden verfügbar.";
      progressBar.style.width = "100%";
      downloadButton.href = x83?.url || data.download_url;
      downloadButton.textContent = x83?.label || "X83 herunterladen";
      downloadButton.hidden = false;
      if (x84) {
        downloadX84Button.href = x84.url;
        downloadX84Button.textContent = x84.label;
        downloadX84Button.hidden = false;
      }
      if (document.body.classList.contains("billing-active")) { reviewButton.href = `/review.html?job_id=${encodeURIComponent(id)}`; reviewButton.hidden = false; }
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
      downloadX84Button.hidden = true;
      downloadX84Button.removeAttribute("href");
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
  downloadX84Button.hidden = true;
  downloadX84Button.removeAttribute("href");
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
