/*!
 * flagships.js — progressive enhancement only.
 * Essential content is readable without JS; every guarded block exits early
 * when its fragment is not on the page.
 */
(function () {
  "use strict";

  /* ------------------------------------------------------------------
     AsterTab — command syntax demo.
     Mirrors the documented parse-command / build-search-url tables.
     Pure local text rendering: no fetch, no model call, no navigation.
     ------------------------------------------------------------------ */
  var ASTER_ROUTES = {
    "": {
      sample: "hello world",
      action: "Bing web search",
      url: "https://www.bing.com/search?q=<query>",
      mode: "Deterministic · no LLM"
    },
    g: {
      sample: "g preact signals",
      action: "Google web search",
      url: "https://www.google.com/search?q=<query>",
      mode: "Deterministic · no LLM"
    },
    gh: {
      sample: "gh aster tab",
      action: "GitHub repository search",
      url: "https://github.com/search?q=<query>&type=repositories",
      mode: "Deterministic · no LLM"
    },
    lg: {
      sample: "lg P1001",
      action: "Luogu problem search",
      url: "https://www.luogu.com.cn/problem/list?keyword=<query>&page=1",
      mode: "Deterministic · no LLM"
    },
    ai: {
      sample: "ai why is my dp wrong",
      action: "Open local learning-diagnostic draft",
      url: "No URL — local draft and request preview",
      mode: "AI only on explicit send · no first-screen network"
    }
  };

  function initAsterDemo() {
    var root = document.querySelector(".flash-aster");
    if (!root) return;
    var buttons = root.querySelectorAll("[data-aster-prefix]");
    if (!buttons.length) return;

    var inputEl = root.querySelector("[data-aster-input]");
    var actionEl = root.querySelector("[data-aster-action]");
    var urlEl = root.querySelector("[data-aster-url]");
    var modeEl = root.querySelector("[data-aster-mode]");

    function render(prefix) {
      var route = ASTER_ROUTES[prefix];
      if (!route) return;
      if (inputEl) inputEl.textContent = route.sample;
      if (actionEl) actionEl.textContent = route.action;
      if (urlEl) urlEl.textContent = route.url;
      if (modeEl) modeEl.textContent = route.mode;
      Array.prototype.forEach.call(buttons, function (btn) {
        btn.setAttribute(
          "aria-pressed",
          btn.getAttribute("data-aster-prefix") === prefix ? "true" : "false"
        );
      });
    }

    Array.prototype.forEach.call(buttons, function (btn) {
      btn.addEventListener("click", function () {
        render(btn.getAttribute("data-aster-prefix") || "");
      });
    });
  }

  function ready(fn) {
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", fn);
    } else {
      fn();
    }
  }

  ready(initAsterDemo);
})();
