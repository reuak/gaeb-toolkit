const params = new URLSearchParams(location.search);
const sessionId = params.get("session_id");
const title = document.querySelector("#status-title");
const copy = document.querySelector("#status-copy");
const paymentStep = document.querySelector("#payment-step");
const emailHint = document.querySelector("#email-hint");
const resend = document.querySelector("#resend");
const note = document.querySelector("#status-note");
let timer;

async function trackPurchase(data) {
  for (let attempt = 0; attempt < 30 && !window.gaebTrackEvent; attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  const purchaseKey = `gaeb-purchase-tracked:${sessionId}`;
  if (sessionStorage.getItem(purchaseKey)) return true;
  let price = null;
  try {
    const billingResponse = await fetch("/api/billing/config");
    if (billingResponse.ok) {
      const billing = await billingResponse.json();
      price = data.offer === "pro_yearly" ? billing.pro_yearly : billing.pro_monthly;
    }
  } catch (_) {
    // Der bestätigte Kauf wird auch ohne optionalen Preiswert gemeldet.
  }
  const tracked = window.gaebTrackEvent?.("purchase", {
    transaction_id: sessionId,
    offer: data.offer,
    value: price ? price.amount / 100 : undefined,
    currency: price?.currency?.toUpperCase(),
    items: [{
      item_id: data.offer,
      item_name: data.offer === "pro_yearly" ? "GAEB Pro Jahresabo" : "GAEB Pro Monatsabo",
    }],
  });
  if (tracked) sessionStorage.setItem(purchaseKey, "1");
  return Boolean(tracked);
}

async function refresh(attempt = 0) {
  if (!sessionId) { showProblem("Die Checkout-ID fehlt. Bitte öffnen Sie den Link aus Ihrer Bestätigung erneut."); return; }
  try {
    const response = await fetch(`/api/billing/checkout-status?session_id=${encodeURIComponent(sessionId)}`);
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || "Status nicht verfügbar.");
    emailHint.textContent = data.email_hint;
    if (data.access_ready) {
      const purchaseTracked = await trackPurchase(data);
      clearTimeout(timer); title.textContent = "Zahlung bestätigt"; paymentStep.textContent = "Abgeschlossen";
      copy.textContent = data.offer.startsWith("pro") ? "GAEB Pro ist aktiviert. Sie werden direkt zum Konverter weitergeleitet." : "Ihre Einzelkonvertierung ist aktiviert.";
      resend.hidden = !data.can_resend; note.textContent = purchaseTracked ? "Zahlung und Kaufereignis wurden bestätigt. Der Zugang ist in diesem Browser 30 Tage aktiv." : "Zahlung bestätigt. Das Kaufereignis wartet auf Ihre Marketing-Einwilligung.";
      timer = setTimeout(() => location.assign("/?billing=ready#preise"), purchaseTracked ? 1200 : 3000); return;
    }
    if (attempt < 20) timer = setTimeout(() => refresh(attempt + 1), 2500);
    else { title.textContent = "Bestätigung dauert länger"; copy.textContent = "Ihre Zahlung ist nicht verloren. Stripe bestätigt sie noch; laden Sie diese Seite in einigen Minuten erneut."; }
  } catch (error) { showProblem(error.message); }
}
function showProblem(message) { title.textContent = "Status derzeit nicht verfügbar"; copy.textContent = message; paymentStep.textContent = "Bitte später erneut prüfen"; }
resend.addEventListener("click", async () => { resend.disabled = true; note.textContent = "Zugangslink wird versendet …";
  const response = await fetch("/api/billing/resend-access", {method:"POST",headers:{"content-type":"application/json"},body:JSON.stringify({session_id:sessionId})});
  note.textContent = response.ok ? "Ein neuer Zugangslink wurde versendet." : ((await response.json()).error || "Versand derzeit nicht möglich.");
  if (!response.ok) resend.disabled = false;
});
refresh();
