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
const gaebReaderForm = document.querySelector("#gaeb-reader-form");
const gaebInput = document.querySelector("#gaeb");
const gaebFileTitle = document.querySelector("#gaeb-file-title");
const gaebFileMeta = document.querySelector("#gaeb-file-meta");

document.querySelector("#year").textContent = new Date().getFullYear();

async function loadImprint() {
  const container = document.querySelector("#imprint-content");
  const source = document.querySelector("#imprint-source");
  try {
    const response = await fetch("/api/legal/imprint");
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || "Impressum nicht verfügbar.");
    container.replaceChildren();
    for (const section of data.sections) {
      const article = document.createElement("article");
      const heading = document.createElement("h3");
      heading.textContent = section.heading;
      article.appendChild(heading);
      for (const line of section.lines) {
        const paragraph = document.createElement("p");
        paragraph.textContent = line;
        article.appendChild(paragraph);
      }
      container.appendChild(article);
    }
    source.href = data.source_url;
  } catch (error) {
    container.textContent = "Die Betreiberangaben konnten nicht automatisch geladen werden. Bitte öffnen Sie das verlinkte Original-Impressum.";
  }
}

loadImprint();

function showFile(file) {
  if (!file) return;
  fileTitle.textContent = file.name;
  fileMeta.textContent = `${(file.size / 1024 / 1024).toFixed(2)} MB`;
}

fileInput.addEventListener("change", () => showFile(fileInput.files[0]));
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
  if (file.size > 2 * 1024 * 1024) {
    showError("Die Datei ist größer als 2 MB.");
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

gaebInput.addEventListener("change", () => {
  const file = gaebInput.files[0];
  if (!file) return;
  gaebFileTitle.textContent = file.name;
  gaebFileMeta.textContent = `${(file.size / 1024 / 1024).toFixed(2)} MB`;
});

gaebReaderForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const file = gaebInput.files[0];
  if (!file) return;
  if (file.size > 2 * 1024 * 1024) {
    gaebFileMeta.textContent = "Die Datei ist größer als 2 MB.";
    gaebFileMeta.style.color = "var(--error)";
    return;
  }

  const submit = gaebReaderForm.querySelector("button[type=submit]");
  submit.disabled = true;
  submit.setAttribute("aria-busy", "true");
  gaebFileMeta.style.color = "";
  gaebFileMeta.textContent = "PDF wird erstellt …";
  try {
    const response = await fetch("/api/gaeb-to-pdf", {
      method: "POST",
      body: new FormData(gaebReaderForm),
    });
    if (!response.ok) {
      const data = await response.json();
      throw new Error(data.error || "GAEB-Datei konnte nicht gelesen werden.");
    }
    const blob = await response.blob();
    const url = URL.createObjectURL(blob);
    const link = document.createElement("a");
    link.href = url;
    link.download = `${file.name.replace(/\.[^.]+$/, "") || "leistungsverzeichnis"}.pdf`;
    document.body.appendChild(link);
    link.click();
    link.remove();
    URL.revokeObjectURL(url);
    gaebFileMeta.textContent = "PDF wurde erstellt.";
  } catch (error) {
    gaebFileMeta.textContent = error.message;
    gaebFileMeta.style.color = "var(--error)";
  } finally {
    submit.disabled = false;
    submit.removeAttribute("aria-busy");
  }
});
