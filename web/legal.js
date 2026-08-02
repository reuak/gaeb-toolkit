document.querySelector("#year")?.replaceChildren(String(new Date().getFullYear()));
async function loadImprint() {
  const container = document.querySelector("#imprint-content"); if (!container) return;
  try { const response = await fetch("/api/legal/imprint"); const data = await response.json(); if (!response.ok) throw new Error(); container.replaceChildren();
    for (const section of data.sections) { const article=document.createElement("article"), heading=document.createElement("h2"); heading.textContent=section.heading; article.append(heading);
      for(const line of section.lines){const p=document.createElement("p");p.textContent=line;article.append(p);} container.append(article); }
    document.querySelector("#imprint-source").href=data.source_url;
  } catch (_) { container.textContent="Die Betreiberangaben konnten vorübergehend nicht geladen werden. Bitte öffnen Sie das verlinkte Original-Impressum."; }
} loadImprint();
