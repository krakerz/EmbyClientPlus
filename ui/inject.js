(function () {
  // Confirmed live against a real login + real playback (2026-09-10, see
  // NOTES.md): the player route's hash is `#!/videoosd/videoosd.html`
  // with no query string at all, so the itemId can't come from the hash
  // — instead watch for the `PlaybackInfo` call Emby's own web client
  // necessarily makes to start playback, and pull the itemId out of that
  // request's URL. Robust against internal routing changes since any
  // working Emby client must call this endpoint to get a stream.
  var lastSeenItemId = null;
  var originalFetch = window.fetch;
  if (originalFetch) {
    window.fetch = function (input, init) {
      try {
        var url = typeof input === 'string' ? input : input && input.url;
        var match = url && url.match(/\/Items\/([^/?]+)\/PlaybackInfo/i);
        if (match) lastSeenItemId = match[1];
      } catch (e) {
        // ignore — never let instrumentation break real playback
      }
      return originalFetch.apply(this, arguments);
    };
  }

  function currentItemIdFromHash() {
    if (!/video/i.test(location.hash)) return null;
    return lastSeenItemId;
  }

  function currentCredentials() {
    // Confirmed live: window.ApiClient.accessToken() is a real function
    // exposing the current session's token.
    try {
      if (window.ApiClient && typeof window.ApiClient.accessToken === 'function') {
        return {
          accessToken: window.ApiClient.accessToken(),
          userId:
            typeof window.ApiClient.getCurrentUserId === 'function'
              ? window.ApiClient.getCurrentUserId()
              : null,
        };
      }
    } catch (e) {
      // fall through to the localStorage scan below
    }

    // Fallback only — not confirmed against a real localStorage schema,
    // kept in case window.ApiClient isn't reachable this way on some
    // Emby web client version.
    for (var i = 0; i < localStorage.length; i++) {
      var key = localStorage.key(i);
      try {
        var value = JSON.parse(localStorage.getItem(key));
        if (value && typeof value.AccessToken === 'string') {
          return { accessToken: value.AccessToken, userId: value.UserId || null };
        }
      } catch (e) {
        // not JSON, or not the credential blob — keep scanning
      }
    }
    return null;
  }

  function muteUnderlyingVideo() {
    var video = document.querySelector('video');
    if (video) {
      video.pause();
      video.muted = true;
    }
  }

  var lastReportedItemId = null;

  function report() {
    var itemId = currentItemIdFromHash();
    if (!itemId) {
      lastReportedItemId = null;
      return;
    }
    if (itemId === lastReportedItemId) return; // already handed off

    var creds = currentCredentials();
    if (!creds || !creds.accessToken) return;

    muteUnderlyingVideo();
    lastReportedItemId = itemId;

    window.__TAURI__.core
      .invoke('intercept_playback', {
        itemId: itemId,
        accessToken: creds.accessToken,
        userId: creds.userId,
      })
      .catch(function () {
        lastReportedItemId = null; // let a later poll retry
      });
  }

  window.addEventListener('hashchange', report);
  setInterval(report, 500);
})();
