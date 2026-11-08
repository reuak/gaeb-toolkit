(async () => {
  const banner = document.querySelector("#offer-banner");
  try {
    const response = await fetch("/api/billing/config");
    if (!response.ok) return;
    const config = await response.json();
    const renderPrice = (kind, price) => {
      const card = document.querySelector(`[data-price-card="${kind}"]`);
      if (!card || !price) return;
      const formatted = new Intl.NumberFormat("de-DE", {
        style: "currency",
        currency: price.currency.toUpperCase(),
      }).format(price.amount / 100);
      document.querySelectorAll(`[data-price-plan="${kind}"]`).forEach((element) => {
        element.textContent = formatted;
      });
      document.querySelectorAll(`[data-price-period="${kind}"]`).forEach((element) => {
        element.textContent = price.interval === "year" ? "netto / Jahr" : "netto / Monat";
      });
      card.hidden = false;
    };
    renderPrice("monthly", config.pro_monthly);
    renderPrice("yearly", config.pro_yearly);
    if (banner && config.offer_banner) {
      banner.textContent = config.offer_banner;
      banner.hidden = false;
    }
  } catch (_) {
    document.querySelectorAll("[data-price-plan]").forEach((element) => {
      element.textContent = "Preis derzeit nicht verfügbar";
    });
  }
})();
