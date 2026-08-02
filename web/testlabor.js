const form = document.querySelector("#test-form");
const input = document.querySelector("#test-pdfs");
const title = document.querySelector("#test-file-title");
const meta = document.querySelector("#test-file-meta");
const note = document.querySelector("#test-note");
const results = document.querySelector("#test-results");
const list = document.querySelector("#test-result-list");
const summary = document.querySelector("#test-summary");
const objectUrls = [];

input.addEventListener("change", () => {
  const files = [...input.files];
  title.textContent = files.length ? `${files.length} PDF-Datei${files.length === 1 ? "" : "en"} ausgewählt` : "Mehrere PDFs auswählen";
  meta.textContent = files.length ? files.map((file) => file.name).join(" · ") : "Maximal 25 MB je Datei";
});

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const files = [...input.files];
  if (!files.length) return;
  const token = new FormData(form).get("token");
  const button = form.querySelector("button[type=submit]");
  button.disabled = true;
  results.hidden = false;
  list.replaceChildren();
  let completed = 0;
  let failed = 0;

  for (const [index, file] of files.entries()) {
    note.textContent = `${index + 1} von ${files.length}: ${file.name} wird analysiert …`;
    const row = document.createElement("div");
    row.className = "account-row test-result-row";
    const description = document.createElement("div");
    const name = document.createElement("strong");
    name.textContent = file.name;
    const state = document.createElement("span");
    state.textContent = "Parser und Export laufen …";
    description.append(name, state);
    row.append(description);
    list.append(row);
    try {
      if (file.size > 25 * 1024 * 1024) throw new Error("Datei ist größer als 25 MB.");
      const body = new FormData();
      body.append("pdf", file, file.name);
      const response = await fetch("/api/admin/test-convert", {
        method: "POST",
        headers: { "x-admin-token": token },
        body,
      });
      if (!response.ok) {
        const data = await response.json().catch(() => ({}));
        throw new Error(data.error || "Testkonvertierung fehlgeschlagen.");
      }
      const blob = await response.blob();
      const url = URL.createObjectURL(blob);
      objectUrls.push(url);
      const download = document.createElement("a");
      download.className = "secondary-button";
      download.href = url;
      download.download = `${file.name.replace(/\.[^.]+$/, "") || "gaeb-test"}-gaeb-test.zip`;
      download.textContent = "ZIP herunterladen";
      row.append(download);
      state.textContent = `Fertig · ${(blob.size / 1024 / 1024).toFixed(2)} MB`;
      completed += 1;
    } catch (error) {
      state.textContent = error.message;
      row.classList.add("is-error");
      failed += 1;
    }
    summary.textContent = `${completed} erfolgreich · ${failed} fehlgeschlagen · ${files.length - index - 1} offen`;
  }
  note.textContent = `Testlauf abgeschlossen: ${completed} erfolgreich, ${failed} fehlgeschlagen.`;
  button.disabled = false;
});

window.addEventListener("beforeunload", () => objectUrls.forEach((url) => URL.revokeObjectURL(url)));
