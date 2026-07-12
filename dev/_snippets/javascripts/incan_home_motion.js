(() => {
  const initialiseHomeMotion = () => {
    const hero = document.querySelector(".inc-hero");
    if (!hero || hero.dataset.motionInitialised === "true") {
      return;
    }

    hero.dataset.motionInitialised = "true";
    document.documentElement.classList.add("inc-motion-capable");

    const flow = hero.querySelector(".inc-hero__flow");
    const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const revealEverything = () => {
      hero.classList.add("inc-motion-visible");
      flow?.classList.add("inc-flow-visible");
    };
    if (reducedMotion || !("IntersectionObserver" in window)) {
      revealEverything();
      return;
    }

    const revealWhenVisible = (element, className, threshold) => {
      if (!element) {
        return;
      }

      const observer = new IntersectionObserver(
        (entries) => {
          if (!entries.some((entry) => entry.isIntersecting)) {
            return;
          }

          element.classList.add(className);
          observer.disconnect();
        },
        { threshold },
      );
      observer.observe(element);
    };

    revealWhenVisible(hero, "inc-motion-visible", 0.24);
    revealWhenVisible(flow, "inc-flow-visible", 0.38);
  };

  if (typeof document$ !== "undefined") {
    document$.subscribe(initialiseHomeMotion);
  } else if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", initialiseHomeMotion, { once: true });
  } else {
    initialiseHomeMotion();
  }
})();
