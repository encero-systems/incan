/* Shared progressive enhancement for Phase 2 docs components. */
(function () {
  const FALLBACK_INCUS_POOLS = Object.freeze({
    tip: ["/shared/incapunk/incus_ui_neutral_001.png"],
    hint: ["/shared/incapunk/incus_ui_neutral_001.png"],
    info: ["/shared/incapunk/incus_ui_neutral_001.png"],
    warning: ["/shared/incapunk/incus_ui_neutral_001.png"],
    python: ["/shared/incapunk/incus_ui_neutral_001.png"],
    rust: ["/shared/incapunk/incus_ui_neutral_001.png"],
    javascript: ["/shared/incapunk/incus_ui_neutral_001.png"],
    "composed-failure": ["/shared/incapunk/incus_ui_neutral_001.png"],
    success: ["/shared/incapunk/incus_ui_success_001.png"],
  });
  const INCUS_POOLS = window.INCAN_INCUS_POOLS || FALLBACK_INCUS_POOLS;

  const ADMONITION_CATEGORIES = Object.freeze({
    tip: "tip",
    hint: "hint",
    info: "info",
    note: "info",
    abstract: "info",
    important: "warning",
    warning: "warning",
    caution: "warning",
    danger: "warning",
    failure: "composed-failure",
    bug: "composed-failure",
    success: "success",
    check: "success",
  });

  function hash(value) {
    let result = 2166136261;
    for (let index = 0; index < value.length; index += 1) {
      result ^= value.charCodeAt(index);
      result = Math.imul(result, 16777619);
    }
    return result >>> 0;
  }

  function collectIncusSlots() {
    document.querySelectorAll(".md-typeset .admonition, .md-typeset details").forEach((element) => {
      if (element.hasAttribute("data-incus-category")) return;
      const category = Object.entries(ADMONITION_CATEGORIES).find(([className]) => (
        element.classList.contains(className)
      ));
      if (!category) return;
      element.classList.add("inc-incus-slot");
      element.setAttribute("data-incus-category", category[1]);
    });
    return Array.from(document.querySelectorAll("[data-incus-category]"));
  }

  function setupIncus() {
    const slots = collectIncusSlots();
    if (slots.length === 0) return;

    // Incus is an Easter egg, not repeated furniture: one contextual sighting per page.
    const selected = slots[hash(window.location.pathname) % slots.length];
    const category = selected.getAttribute("data-incus-category");
    const seasonalPool = new Date().getMonth() === 9
      ? (INCUS_POOLS["seasonal-october"] || [])
      : [];
    const easterEggPool = [...(INCUS_POOLS["easter-egg"] || []), ...seasonalPool];
    const shouldUseEasterEgg = ["tip", "hint", "info", "neutral"].includes(category)
      && easterEggPool.length > 0
      && hash(`${window.location.pathname}:easter-egg`) % 7 === 0;
    const pool = shouldUseEasterEgg ? easterEggPool : INCUS_POOLS[category];
    if (!pool || pool.length === 0 || selected.dataset.incusBound === "true") return;

    selected.dataset.incusBound = "true";
    selected.classList.add("inc-incus-slot--active");
    const asset = pool[hash(`${window.location.pathname}:${category}`) % pool.length];
    const image = document.createElement("img");
    image.className = "inc-incus-slot__image";
    image.src = asset;
    image.alt = "";
    image.setAttribute("aria-hidden", "true");
    image.decoding = "async";
    image.loading = "lazy";
    selected.appendChild(image);
  }

  function setupNavigation() {
    const drawer = document.querySelector('label.md-header__button[for="__drawer"]');
    const control = document.getElementById("__drawer");
    if (!drawer || !control || drawer.dataset.incanNavigationBound === "true") return;
    drawer.dataset.incanNavigationBound = "true";
    drawer.setAttribute("role", "button");
    drawer.tabIndex = 0;

    const updateState = () => {
      const expanded = control.checked;
      const action = expanded ? "Close navigation" : "Open navigation";
      drawer.setAttribute("aria-label", action);
      drawer.setAttribute("title", action);
      drawer.setAttribute("aria-expanded", String(expanded));
    };

    drawer.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      drawer.click();
    });
    control.addEventListener("change", updateState);
    updateState();
  }

  function init() {
    setupNavigation();
    document.body.classList.toggle(
      "inc-reference-page",
      window.location.pathname.includes("/language/reference/"),
    );
    setupIncus();
  }

  if (typeof window.document$ !== "undefined" && typeof window.document$.subscribe === "function") {
    window.document$.subscribe(init);
  } else {
    document.addEventListener("DOMContentLoaded", init);
  }
})();
