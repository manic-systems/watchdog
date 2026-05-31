// Watchdog Analytics Beacon
(function () {
  "use strict";

  // Configuration from script tag data attributes
  var scriptEl = document.currentScript;
  var config = {
    endpoint: scriptEl.getAttribute("data-api") || "/api/event",
    domain: scriptEl.getAttribute("data-domain") || window.location.hostname,
    hashMode: scriptEl.hasAttribute("data-hash-mode"),
    outboundLinks: scriptEl.hasAttribute("data-outbound-links"),
    fileDownloads: scriptEl.hasAttribute("data-file-downloads"),
    exclude: scriptEl.getAttribute("data-exclude") || "",
    manual: scriptEl.hasAttribute("data-manual"),
  };

  var tracked = false;
  var lastPage = null;
  var sessionStarted = false;
  var engagementStartedAt = Date.now();
  var engagementMs = 0;
  var maxScrollDepth = 0;

  // Parse exclusions (comma-separated paths)
  var exclusions = config.exclude
    ? config.exclude.split(",").map(function (s) {
        return s.trim();
      })
    : [];

  // Check if page should be tracked
  function shouldTrack() {
    // Skip localhost unless explicitly allowed
    if (
      window.location.hostname === "localhost" ||
      window.location.hostname === "127.0.0.1"
    ) {
      return false;
    }

    // Check exclusions
    var path = window.location.pathname;
    for (var i = 0; i < exclusions.length; i++) {
      if (path.indexOf(exclusions[i]) === 0) {
        return false;
      }
    }

    return true;
  }

  // Send analytics payload to server
  function sendBeacon(payload) {
    if (!shouldTrack()) return;

    var data = JSON.stringify(payload);

    // Try navigator.sendBeacon first (best for page unload)
    if (navigator.sendBeacon) {
      var blob = new Blob([data], { type: "application/json" });
      navigator.sendBeacon(config.endpoint, blob);
      return;
    }

    // Fallback to fetch for browsers without sendBeacon
    if (window.fetch) {
      fetch(config.endpoint, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: data,
        keepalive: true,
      }).catch(function () {
        // Silently fail, analytics shouldn't break the page
      });
      return;
    }

    // Final fallback to XMLHttpRequest
    try {
      var xhr = new XMLHttpRequest();
      xhr.open("POST", config.endpoint, true);
      xhr.setRequestHeader("Content-Type", "application/json");
      xhr.send(data);
    } catch (e) {
      // Silently fail
    }
  }

  function absoluteUrl(path) {
    if (!path) return window.location.href;
    try {
      return new URL(path, window.location.href).href;
    } catch (e) {
      return window.location.href;
    }
  }

  function sessionMarker() {
    if (sessionStarted) return false;

    var key = "watchdog:session:" + config.domain;
    try {
      if (window.sessionStorage.getItem(key)) {
        sessionStarted = true;
        return false;
      }
      window.sessionStorage.setItem(key, "1");
    } catch (e) {
      // Fall back to an in-memory marker when sessionStorage is unavailable.
    }

    sessionStarted = true;
    return true;
  }

  function cleanProps(props) {
    if (!props || typeof props !== "object") return null;

    var cleaned = {};
    var count = 0;
    for (var key in props) {
      if (!Object.prototype.hasOwnProperty.call(props, key)) continue;
      if (count >= 20) break;

      var cleanKey = String(key).slice(0, 64);
      if (!cleanKey) continue;

      var value = props[key];
      if (typeof value === "string") {
        value = value.slice(0, 200);
      } else if (typeof value !== "number" && typeof value !== "boolean") {
        if (value === null || value === undefined) continue;
        value = String(value).slice(0, 200);
      }

      cleaned[cleanKey] = value;
      count++;
    }

    return count ? cleaned : null;
  }

  // Build payload
  function buildPayload(opts) {
    opts = opts || {};
    var payload = {
      d: config.domain,
      u: opts.url || absoluteUrl(opts.path),
      r: opts.referrer !== undefined ? opts.referrer : document.referrer || "",
      w: window.screen.width || 0,
    };

    if (opts.name) payload.n = opts.name;
    var props = cleanProps(opts.props);
    if (props) payload.p = props;
    if (opts.engagementSeconds) payload.e = opts.engagementSeconds;
    if (opts.scrollDepth) payload.sd = opts.scrollDepth;
    if (opts.session) payload.s = true;

    return payload;
  }

  // Track a pageview
  function trackPageview(opts) {
    opts = opts || {};

    // Get current page (with hash if hash-mode is enabled)
    var currentPage = window.location.pathname + window.location.search;
    if (config.hashMode) {
      currentPage += window.location.hash;
    }

    // Avoid duplicate pageviews
    if (lastPage === currentPage && !opts.force) {
      return;
    }

    lastPage = currentPage;
    tracked = true;

    var payload = buildPayload({
      path: currentPage,
      name: "pageview",
      referrer: opts.referrer,
      session: sessionMarker(),
    });

    sendBeacon(payload);
  }

  // Track a custom event
  function trackEvent(eventName, opts) {
    if (!eventName || typeof eventName !== "string") {
      console.warn("Watchdog: event name must be a non-empty string");
      return;
    }

    opts = opts || {};
    var payload = buildPayload(opts);
    payload.n = eventName;
    sendBeacon(payload);
  }

  function updateEngagement() {
    if (document.hidden) return;

    var now = Date.now();
    engagementMs += now - engagementStartedAt;
    engagementStartedAt = now;
  }

  function updateScrollDepth() {
    var doc = document.documentElement;
    var body = document.body;
    var scrollTop = window.pageYOffset || doc.scrollTop || body.scrollTop || 0;
    var viewport = window.innerHeight || doc.clientHeight || 0;
    var height = Math.max(
      body.scrollHeight,
      body.offsetHeight,
      doc.clientHeight,
      doc.scrollHeight,
      doc.offsetHeight
    );

    if (!height || height <= viewport) {
      maxScrollDepth = Math.max(maxScrollDepth, 100);
      return;
    }

    var depth = Math.round(((scrollTop + viewport) / height) * 100);
    maxScrollDepth = Math.max(maxScrollDepth, Math.min(100, depth));
  }

  function sendEngagement() {
    if (!tracked) return;

    updateEngagement();
    updateScrollDepth();

    if (engagementMs < 1000 && maxScrollDepth === 0) return;

    var seconds = Math.round(engagementMs / 100) / 10;
    var payload = buildPayload({
      name: "engagement",
      engagementSeconds: seconds,
      scrollDepth: maxScrollDepth,
    });

    engagementMs = 0;
    maxScrollDepth = 0;
    sendBeacon(payload);
  }

  // Track outbound link clicks
  function trackOutboundLink(event) {
    var link = event.target.closest("a");
    if (!link) return;

    var url = link.getAttribute("href");
    if (!url || url.indexOf("://") === -1) return;

    // Check if external
    var linkHostname = link.hostname;
    if (linkHostname === window.location.hostname) return;

    // Track as custom event
    trackEvent("Outbound Link: Click", {
      path: window.location.pathname,
      props: { url: url },
    });
  }

  // Track file downloads
  function trackFileDownload(event) {
    var link = event.target.closest("a");
    if (!link) return;

    var href = link.getAttribute("href");
    if (!href) return;

    // Common file extensions.
    // FIXME: this needs to be more robust in the future
    var extensions = [
      ".pdf",
      ".zip",
      ".tar",
      ".gz",
      ".doc",
      ".docx",
      ".xls",
      ".xlsx",
      ".ppt",
      ".pptx",
    ];

    var isDownload = false;
    for (var i = 0; i < extensions.length; i++) {
      if (href.toLowerCase().indexOf(extensions[i]) !== -1) {
        isDownload = true;
        break;
      }
    }

    if (!isDownload) return;

    // Track as custom event
    trackEvent("File Download", {
      path: window.location.pathname,
      props: { url: href },
    });
  }

  // Setup automatic tracking features
  function setupAutomaticTracking() {
    // Hash mode, track when hash changes (for SPAs)
    if (config.hashMode) {
      window.addEventListener("hashchange", function () {
        trackPageview();
      });
    }

    // Outbound link tracking
    if (config.outboundLinks) {
      document.addEventListener("click", trackOutboundLink);
    }

    // File download tracking
    if (config.fileDownloads) {
      document.addEventListener("click", trackFileDownload);
    }

    window.addEventListener("scroll", updateScrollDepth, { passive: true });
    window.addEventListener("pagehide", sendEngagement);
    document.addEventListener("visibilitychange", function () {
      if (document.hidden) {
        sendEngagement();
      } else {
        engagementStartedAt = Date.now();
      }
    });
  }

  // Expose public API
  window.watchdog = {
    track: trackEvent,
    trackPageview: trackPageview,
  };

  // Auto-track pageview on load (unless manual mode)
  if (!config.manual) {
    if (document.readyState === "complete") {
      trackPageview();
    } else {
      window.addEventListener("load", trackPageview);
    }
  }

  // Setup automatic tracking features
  setupAutomaticTracking();
})();
