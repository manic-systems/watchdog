// Watchdog Analytics Beacon
(function () {
  "use strict";

  // Configuration from script tag data attributes
  var scriptEl = document.currentScript || lastScriptElement();

  function attr(name) {
    return scriptEl ? scriptEl.getAttribute(name) : null;
  }

  function hasAttr(name) {
    return !!(scriptEl && scriptEl.hasAttribute(name));
  }

  var config = {
    endpoint:
      attr("data-api") ||
      new URL(
        "/api/event",
        (scriptEl && scriptEl.src) || window.location.href,
      ).href,
    domain: attr("data-domain") || window.location.hostname,
    hashMode: hasAttr("data-hash-mode"),
    outboundLinks: hasAttr("data-outbound-links"),
    fileDownloads: hasAttr("data-file-downloads"),
    exclude: attr("data-exclude") || "",
    manual: hasAttr("data-manual"),
    allowLocalhost: hasAttr("data-allow-localhost"),
    engagement: !hasAttr("data-disable-engagement"),
  };

  var sameOriginEndpoint =
    new URL(config.endpoint, document.baseURI || window.location.href).origin ===
    window.location.origin;

  var pageview = null;
  var lastPage = null;
  var sessionStarted = false;
  var engagementStartedAt = null;
  var engagementMs = 0;
  var maxScrollDepth = 0;
  var reportedScrollDepth = 0;
  var scrollQueued = false;

  function lastScriptElement() {
    var scripts = document.getElementsByTagName("script");
    return scripts.length ? scripts[scripts.length - 1] : null;
  }

  // Parse exclusions (comma-separated paths)
  var exclusions = config.exclude
    ? config.exclude
        .split(",")
        .map(function (s) {
          return s.trim();
        })
        .filter(function (s) {
          return s.length > 0;
        })
    : [];

  // Check if page should be tracked
  function shouldTrack(url) {
    // Skip localhost unless explicitly allowed
    if (
      (window.location.hostname === "localhost" ||
        window.location.hostname === "127.0.0.1") &&
      !config.allowLocalhost
    ) {
      return false;
    }

    // Check exclusions
    var path = window.location.pathname;
    var reportedPath;
    try {
      reportedPath = new URL(url, window.location.href).pathname;
    } catch (e) {
      return false;
    }

    for (var i = 0; i < exclusions.length; i++) {
      if (
        path.indexOf(exclusions[i]) === 0 ||
        reportedPath.indexOf(exclusions[i]) === 0
      ) {
        return false;
      }
    }

    return true;
  }

  // Send analytics payload to server
  function sendBeacon(payload) {
    var data = JSON.stringify(payload);

    // Try navigator.sendBeacon first (best for page unload)
    if (navigator.sendBeacon && sameOriginEndpoint) {
      try {
        var blob = new Blob([data], { type: "application/json" });
        if (navigator.sendBeacon(config.endpoint, blob)) return true;
      } catch (e) {
        // Fall through to fetch/XMLHttpRequest.
      }
    }

    // Fallback to fetch for browsers without sendBeacon
    if (window.fetch) {
      fetch(config.endpoint, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: data,
        credentials: "omit",
        keepalive: true,
      }).catch(function () {
        // Silently fail, analytics shouldn't break the page
      });
      return true;
    }

    // Final fallback to XMLHttpRequest
    try {
      var xhr = new XMLHttpRequest();
      xhr.open("POST", config.endpoint, true);
      xhr.setRequestHeader("Content-Type", "application/json");
      xhr.send(data);
      return true;
    } catch (e) {
      // Silently fail
    }

    return false;
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
      if (count >= 10) break;

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
    var currentPage =
      opts.path || window.location.pathname + window.location.search;
    if (config.hashMode) {
      currentPage += window.location.hash;
    }

    var currentUrl = absoluteUrl(currentPage);
    var canTrack = shouldTrack(currentUrl);

    // Avoid duplicate pageviews
    if (canTrack && lastPage === currentPage && !opts.force) {
      return;
    }

    sendEngagement();
    pageview = null;
    lastPage = null;
    engagementStartedAt = null;
    engagementMs = 0;
    maxScrollDepth = 0;
    reportedScrollDepth = 0;

    if (!canTrack) return;

    var payload = buildPayload({
      url: currentUrl,
      name: "pageview",
      referrer: opts.referrer,
      session: sessionMarker(),
    });

    if (sendBeacon(payload)) {
      lastPage = currentPage;
      pageview = payload;

      if (config.engagement && !document.hidden) {
        engagementStartedAt = Date.now();
        updateScrollDepth();
      }
    }
  }

  // Track a custom event
  function trackEvent(eventName, opts) {
    if (!eventName || typeof eventName !== "string") {
      console.warn("Watchdog: event name must be a non-empty string");
      return;
    }

    opts = opts || {};
    var payload = buildPayload(opts);
    if (!shouldTrack(payload.u)) return;

    payload.n = eventName;
    sendBeacon(payload);
  }

  function scheduleScrollDepth() {
    if (scrollQueued) return;

    scrollQueued = true;
    var schedule =
      window.requestAnimationFrame ||
      function (callback) {
        return setTimeout(callback, 100);
      };
    schedule(function () {
      scrollQueued = false;
      updateScrollDepth();
    });
  }

  function updateEngagement() {
    if (engagementStartedAt === null) return;

    var now = Date.now();
    engagementMs += now - engagementStartedAt;
    engagementStartedAt = document.hidden ? null : now;
  }

  function updateScrollDepth() {
    if (!pageview || engagementStartedAt === null) return;

    var doc = document.documentElement;
    var body = document.body || doc;
    var scrollTop = window.pageYOffset || doc.scrollTop || body.scrollTop || 0;
    var viewport = window.innerHeight || doc.clientHeight || 0;
    var height = Math.max(
      body.scrollHeight,
      body.offsetHeight,
      doc.clientHeight,
      doc.scrollHeight,
      doc.offsetHeight,
    );

    if (!height || height <= viewport) {
      maxScrollDepth = Math.max(maxScrollDepth, 100);
      return;
    }

    var depth = Math.round(((scrollTop + viewport) / height) * 100);
    maxScrollDepth = Math.max(maxScrollDepth, Math.min(100, depth));
  }

  function sendEngagement() {
    if (!pageview || !config.engagement) return;

    updateEngagement();
    var scrollDepth = maxScrollDepth > reportedScrollDepth ? maxScrollDepth : 0;

    if (engagementMs < 1000 && scrollDepth === 0) return;

    var seconds = Math.round(engagementMs / 100) / 10;
    var payload = buildPayload({
      url: pageview.u,
      referrer: pageview.r,
      name: "engagement",
      engagementSeconds: seconds,
      scrollDepth: scrollDepth,
    });

    if (sendBeacon(payload)) {
      engagementMs = 0;
      reportedScrollDepth = maxScrollDepth;
    }
  }

  function pauseEngagement() {
    updateScrollDepth();
    sendEngagement();
    engagementStartedAt = null;
  }

  // Track outbound link clicks
  function trackOutboundLink(event) {
    var link = closestLink(event.target);
    if (!link) return;

    var url;
    try {
      url = new URL(link.href, window.location.href);
    } catch (e) {
      return;
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") return;

    // Check if external
    if (url.hostname === window.location.hostname) return;

    // Track as custom event
    trackEvent("Outbound Link: Click", {
      path: window.location.pathname,
      props: { url: url.href },
    });
  }

  // Track file downloads
  function trackFileDownload(event) {
    var link = closestLink(event.target);
    if (!link) return;

    var url;
    try {
      url = new URL(link.href, window.location.href);
    } catch (e) {
      return;
    }
    if (url.protocol !== "http:" && url.protocol !== "https:") return;

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
    var path = url.pathname.toLowerCase();
    for (var i = 0; i < extensions.length; i++) {
      if (path.slice(-extensions[i].length) === extensions[i]) {
        isDownload = true;
        break;
      }
    }

    if (!isDownload) return;

    // Track as custom event
    trackEvent("File Download", {
      path: window.location.pathname,
      props: { url: url.href },
    });
  }

  function closestLink(target) {
    while (target && target !== document) {
      if (target.tagName && target.tagName.toLowerCase() === "a") return target;
      target = target.parentElement;
    }
    return null;
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

    if (config.engagement) {
      window.addEventListener("scroll", scheduleScrollDepth, { passive: true });
      window.addEventListener("pagehide", pauseEngagement);
      document.addEventListener("visibilitychange", function () {
        if (document.hidden) {
          pauseEngagement();
        } else if (pageview) {
          engagementStartedAt = Date.now();
          updateScrollDepth();
        }
      });
    }
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
