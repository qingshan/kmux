/*
 * Shared ES5 helpers for qingshan Mesquite WAFs.
 *
 * Loaded before each package's script.js. No modules, no fetch, no
 * const/let/arrows. Mesquite's WebKit is WebKitGTK 1.0.7.2.
 */

function getKindle() {
    try {
        if (typeof window !== "undefined" && window.kindle) {
            return window.kindle;
        }
        if (typeof top !== "undefined" && top.kindle) {
            return top.kindle;
        }
    } catch (e) {
        return window.kindle || null;
    }
    return null;
}

function byId(id) {
    return document.getElementById(id);
}

function esc(s) {
    return String(s === undefined || s === null ? "" : s)
        .replace(/&/g, "&amp;")
        .replace(/</g, "&lt;")
        .replace(/>/g, "&gt;")
        .replace(/"/g, "&quot;");
}

function setState(text, cssClass) {
    var el = byId("state");
    if (el) {
        el.className = "state " + cssClass;
        el.innerHTML = text;
    }
}

function setError(text) {
    var el = byId("error");
    if (el) {
        el.innerHTML = text || "";
    }
}

function sendJsonCmd(appId, op) {
    var kindle = getKindle();
    if (!kindle || !kindle.messaging || typeof kindle.messaging.sendStringMessage !== "function") {
        setError("Kindle messaging API unavailable - is this running inside Mesquite?");
        return false;
    }
    try {
        kindle.messaging.sendStringMessage(appId, "cmd", JSON.stringify(op));
        return true;
    } catch (err) {
        setError("Failed to reach " + appId + " (" + (err.message || err) + ")");
        return false;
    }
}

function sendStringCmd(appId, property, cmd) {
    var kindle = getKindle();
    if (!kindle || !kindle.messaging || typeof kindle.messaging.sendStringMessage !== "function") {
        setError("Kindle messaging API unavailable - is this running inside Mesquite?");
        return false;
    }
    try {
        kindle.messaging.sendStringMessage(appId, property, cmd);
        return true;
    } catch (err) {
        setError("Failed to reach " + appId + " (" + (err.message || err) + ")");
        return false;
    }
}

function pollStatusJson(onData, missingMsg) {
    var xhr = new XMLHttpRequest();
    try {
        xhr.open("GET", "status.json?t=" + new Date().getTime(), true);
    } catch (err) {
        setError("Could not open status.json");
        return;
    }
    xhr.onreadystatechange = function () {
        if (xhr.readyState !== 4) {
            return;
        }
        if (xhr.status !== 0 && xhr.status !== 200) {
            setError("status.json not readable (HTTP " + xhr.status + ")");
            return;
        }
        if (!xhr.responseText || !xhr.responseText.replace(/\s/g, "")) {
            setError(missingMsg || "status.json missing - is the daemon running? Tap the Home screen icon to reopen.");
            return;
        }
        var data;
        try {
            data = JSON.parse(xhr.responseText);
        } catch (err) {
            setError("Could not parse status.json");
            return;
        }
        onData(data);
    };
    try {
        xhr.send(null);
    } catch (err) {
        setError("Could not read status.json");
    }
}

function updateChrome(appId, title) {
    var kindle = getKindle();
    if (!kindle || !kindle.messaging || typeof kindle.messaging.sendMessage !== "function") {
        return;
    }
    var chromebar = {
        appId: appId,
        topNavBar: {
            template: "title",
            title: title,
            buttons: [
                { id: "KPP_CLOSE", state: "enabled", handling: "system" }
            ]
        }
    };
    try {
        kindle.messaging.sendMessage("com.lab126.chromebar", "configureChrome", chromebar);
    } catch (err) {
        /* Non-fatal - the WAF still works without a chrome bar. */
    }
}

function hookChromeOnGo(appId, title) {
    var kindle = getKindle();
    if (kindle) {
        kindle.appmgr = kindle.appmgr || {};
        kindle.appmgr.ongo = function () {
            updateChrome(appId, title);
        };
    }
    updateChrome(appId, title);
}

function startAutoRefresh(fn, intervalMs) {
    if (typeof refreshTimer !== "undefined" && refreshTimer) {
        clearInterval(refreshTimer);
    }
    refreshTimer = setInterval(fn, intervalMs);
}
