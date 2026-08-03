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
    const singleRegularPrice = document.querySelector("#single-regular-price");
    const proRegularPrice = document.querySelector("#pro-regular-price");
    if (singlePrice) singlePrice.textContent = money(config.single_net_cents);
    if (proPrice) proPrice.textContent = money(config.pro_net_cents);
    if (singleRegularPrice) singleRegularPrice.textContent = money(config.regular_single_net_cents);
    if (proRegularPrice) proRegularPrice.textContent = money(config.regular_pro_net_cents);
    const toggleOffer = (kind, current, regular) => {
      const active = current < regular;
      const label = document.querySelector(`#${kind}-offer-label`);
      const row = document.querySelector(`#${kind}-regular-row`);
      if (label) label.hidden = !active;
      if (row) row.hidden = !active;
    };
    toggleOffer("single", config.single_net_cents, config.regular_single_net_cents);
    toggleOffer("pro", config.pro_net_cents, config.regular_pro_net_cents);
    if (banner && config.offer_banner) {
      banner.textContent = config.offer_banner;
      banner.hidden = false;
    }
  } catch (_) {
    // Preise im HTML bleiben als verständliche Rückfallwerte sichtbar.
  }
})();
