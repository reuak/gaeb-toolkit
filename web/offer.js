(async () => {
  const banner = document.querySelector("#offer-banner");
  try {
    const response = await fetch("/api/billing/config");
    if (!response.ok) return;
    const config = await response.json();
    const money = (cents) =>
      new Intl.NumberFormat("de-DE", { style: "currency", currency: "EUR" }).format(cents / 100);
    const singlePrice = document.querySelector("#single-price");
    const proPrice = document.querySelector("#pro-price");
    if (singlePrice) singlePrice.textContent = money(config.single_net_cents);
    if (proPrice) proPrice.textContent = money(config.pro_net_cents);
    if (banner && config.offer_banner) {
      banner.textContent = config.offer_banner;
      banner.hidden = false;
    }
  } catch (_) {
    // Preise im HTML bleiben als verständliche Rückfallwerte sichtbar.
  }
})();
