const params = new URLSearchParams(location.search);
const sessionId = params.get("session_id");
const title = document.querySelector("#status-title");
const copy = document.querySelector("#status-copy");
const paymentStep = document.querySelector("#payment-step");
const emailHint = document.querySelector("#email-hint");
const resend = document.querySelector("#resend");
const note = document.querySelector("#status-note");
let timer;

async function refresh(attempt = 0) {
  if (!sessionId) { showProblem("Die Checkout-ID fehlt. Bitte öffnen Sie den Link aus Ihrer Bestätigung erneut."); return; }
  try {
    const response = await fetch(`/api/billing/checkout-status?session_id=${encodeURIComponent(sessionId)}`);
    const data = await response.json();
    if (!response.ok) throw new Error(data.error || "Status nicht verfügbar.");
    emailHint.textContent = data.email_hint;
    if (data.access_ready) {
      clearTimeout(timer); title.textContent = "Zahlung bestätigt"; paymentStep.textContent = "Abgeschlossen";
      copy.textContent = data.offer === "pro" ? "GAEB Pro ist eingerichtet. Öffnen Sie jetzt den Magic-Link in Ihrer E-Mail." : "Ihre Einzelkonvertierung wurde Ihrem sicheren E-Mail-Konto gutgeschrieben.";
      resend.hidden = !data.can_resend; note.textContent = "Keine E-Mail? Prüfen Sie bitte auch den Spam-Ordner."; return;
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
